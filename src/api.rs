//! The page's JSON API under `/app/rc-talk/api/`. People are named by account
//! id (`id`), except a new contact request, which can only name a username.
//!
//! | request                              | body                       | what                                  |
//! |--------------------------------------|----------------------------|---------------------------------------|
//! | `GET  /state`                        |                            | me, contacts, asked, rooms, what works |
//! | `POST /contacts`                     | `{username}`               | ask (or accept, if they asked)        |
//! | `POST /contacts/accept`              | `{id}`                     | accept a request                      |
//! | `POST /contacts/block`               | `{id}`                     | take nothing of theirs                |
//! | `POST /contacts/remove`              | `{id}`                     | forget (a blocked one is unblocked)   |
//! | `POST /asked/cancel`                 | `{username}`               | forget a request of mine              |
//! | `POST /rooms`                        | `{title, members: [id]}`   | new group of accepted contacts        |
//! | `POST /rooms/<id>/members`           | `{title, members: [id]}`   | the owner changes the group           |
//! | `POST /rooms/<id>/leave`             |                            | leave a group                         |
//! | `GET  /rooms/<id>/messages?before=`  |                            | `{messages, older}`, by whole days    |
//! | `POST /rooms/<id>/messages`          | `{text}`                   | send                                  |
//! | `POST /rooms/<id>/edit`              | `{msg, at, text}`          | change my message                     |
//! | `POST /rooms/<id>/delete`            | `{msg, at}`                | delete my message                     |
//! | `POST /rooms/<id>/read`              |                            | nothing unread                        |
//! | `POST /rooms/<id>/call`              |                            | `{url, token, room}` for LiveKit      |
//!
//! A change is checked on the state as loaded, sent, and then made with
//! [`store::update`], which may make it again on a newer state (a delivery
//! came in between): sending happens once, outside it.

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::delivery;
use crate::inbox::{State, MAX_MEMBERS};
use crate::model::*;
use crate::rustcloud::app::{http, jobs, log, settings};
use crate::store;
use crate::{livekit, random_id, Response, User};

type Failure = (u16, &'static str);
type Answer = Result<Value, Failure>;

const BAD: Failure = (400, "BAD_REQUEST");
const NOT_FOUND: Failure = (404, "NOT_FOUND");
const FORBIDDEN: Failure = (403, "FORBIDDEN");
const TOO_MANY: Failure = (429, "TOO_MANY");

pub fn handle(user: &User, method: &str, path: &str, query: &str, body: &[u8]) -> Response {
    match route(user, method, path, query, body) {
        Ok(value) => crate::json(200, &value),
        Err((status, code)) => crate::json(status, &json!({ "error": code })),
    }
}

fn route(user: &User, method: &str, path: &str, query: &str, body: &[u8]) -> Answer {
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, segments.as_slice()) {
        ("GET", ["state"]) => state(user),
        ("POST", ["contacts"]) => ask(user, parse::<Named>(body)?.username),
        ("POST", ["contacts", "accept"]) => accept(user, &parse::<Id>(body)?.id),
        ("POST", ["contacts", "block"]) => set_contact(user, &parse::<Id>(body)?.id, Some(ContactState::Blocked)),
        ("POST", ["contacts", "remove"]) => set_contact(user, &parse::<Id>(body)?.id, None),
        ("POST", ["asked", "cancel"]) => cancel_ask(user, &parse::<Named>(body)?.username),
        ("POST", ["rooms"]) => new_group(user, parse(body)?),
        ("POST", ["rooms", id, "members"]) => change_group(user, id, parse(body)?),
        ("POST", ["rooms", id, "leave"]) => leave(user, id),
        ("GET", ["rooms", id, "messages"]) => messages(user, id, &crate::query_param(query, "before")),
        ("POST", ["rooms", id, "messages"]) => send(user, id, &parse::<Text>(body)?.text),
        ("POST", ["rooms", id, "edit"]) => edit(user, id, parse(body)?),
        ("POST", ["rooms", id, "delete"]) => delete(user, id, parse(body)?),
        ("POST", ["rooms", id, "read"]) => read(user, id),
        ("POST", ["rooms", id, "call"]) => call(user, id),
        _ => Err(NOT_FOUND),
    }
}

fn fail(error: String) -> Failure {
    log::warn(&format!("talk: {error}"));
    (500, "FAILED")
}

fn parse<T: for<'a> Deserialize<'a>>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body).map_err(|_| BAD)
}

#[derive(Deserialize)]
struct Named {
    username: String,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Text {
    text: String,
}

#[derive(Deserialize)]
struct Group {
    title: String,
    members: Vec<String>,
}

#[derive(Deserialize)]
struct Change {
    msg: String,
    at: DateTime<Utc>,
    #[serde(default)]
    text: String,
}

fn load(user: &User) -> Result<State, Failure> {
    store::load(delivery::sender(user)).map_err(fail)
}

/// Makes a change to the state (see the module's doc).
fn commit<T>(user: &User, change: impl FnMut(&mut State) -> T) -> Result<T, Failure> {
    store::update(&delivery::sender(user), change).map_err(fail)
}

fn envelope(body: Body, sent_at: DateTime<Utc>) -> Envelope {
    Envelope { v: ENVELOPE_VERSION, id: random_id(), sent_at, body }
}

fn sent(result: Result<(), delivery::Error>) -> Result<(), Failure> {
    match result {
        Ok(()) => Ok(()),
        Err(delivery::Error::TooMany) => Err(TOO_MANY),
        Err(delivery::Error::Failed(e)) => Err(fail(e)),
    }
}

/// Sends a new envelope to account ids.
fn deliver(user: &User, to: &[String], body: Body) -> Result<(), Failure> {
    sent(delivery::send(user, to, &envelope(body, Utc::now())))
}

fn settings_json() -> Value {
    serde_json::from_str(&settings::values()).unwrap_or_default()
}

fn state(user: &User) -> Answer {
    let state = load(user)?;
    delivery::badge(state.waiting());
    Ok(json!({
        "me": { "id": user.id, "username": user.username, "display_name": user.display_name },
        "contacts": state.book.contacts,
        "asked": state.book.asked,
        "rooms": state.rooms,
        "calls": livekit::Config::from_settings(&settings_json()).is_some(),
    }))
}

fn ask(user: &User, username: String) -> Answer {
    let username = username.trim().to_owned();
    if !valid_username(&username) || username == user.username {
        return Err(BAD);
    }
    let state = load(user)?;
    // Someone with this username already in the book: answer by their id.
    let known = state.book.contacts.iter().find(|(_, c)| c.username == username).map(|(id, c)| (id.clone(), c.state));
    match known {
        Some((id, ContactState::Incoming)) => return accept(user, &id),
        Some((_, contact_state)) => return Ok(json!({ "state": contact_state })),
        None => {}
    }
    sent(delivery::send(user, std::slice::from_ref(&username), &envelope(Body::ContactRequest, Utc::now())))?;
    let now = Utc::now();
    commit(user, |state| state.book.asked.insert(username.clone(), now))?;
    Ok(json!({ "state": "asked" }))
}

fn accept(user: &User, id: &str) -> Answer {
    let state = load(user)?;
    state.book.contacts.get(id).filter(|c| c.state == ContactState::Incoming).ok_or(NOT_FOUND)?;
    deliver(user, &[id.to_owned()], Body::ContactAccept)?;
    let now = Utc::now();
    let waiting = commit(user, |state| {
        if let Some(contact) = state.book.contacts.get_mut(id).filter(|c| c.state == ContactState::Incoming) {
            contact.state = ContactState::Accepted;
            contact.since = now;
            let person = contact.person();
            state.direct_room(id, person);
        }
        state.waiting()
    })?;
    delivery::badge(waiting);
    Ok(json!({ "state": ContactState::Accepted }))
}

fn set_contact(user: &User, id: &str, to: Option<ContactState>) -> Answer {
    // Only someone already in the book: their names came from the server.
    let before = load(user)?.book.contacts.get(id).map(|c| c.state).ok_or(NOT_FOUND)?;
    // The server stops what they send too, rings included, and drops what waits.
    let server = match to {
        Some(ContactState::Blocked) => jobs::block(id),
        _ if before == ContactState::Blocked => jobs::unblock(id),
        _ => Ok(()),
    };
    server.map_err(|e| fail(format!("block {id}: {e:?}")))?;
    let waiting = commit(user, |state| {
        match to {
            Some(next) => {
                if let Some(contact) = state.book.contacts.get_mut(id) {
                    contact.state = next;
                }
            }
            None => {
                state.book.contacts.remove(id);
            }
        }
        state.waiting()
    })?;
    delivery::badge(waiting);
    Ok(json!({}))
}

fn cancel_ask(user: &User, username: &str) -> Answer {
    commit(user, |state| state.book.asked.remove(username.trim()))?;
    Ok(json!({}))
}

/// How the owner describes members to each other.
fn describe(state: &State, room: Option<&Room>, ids: &[String]) -> Vec<Member> {
    ids.iter()
        .map(|id| {
            let person = if *id == state.me.id {
                Person::from(&state.me)
            } else if let Some(contact) = state.book.contacts.get(id) {
                contact.person()
            } else {
                room.and_then(|r| r.names.get(id).cloned()).unwrap_or_default()
            };
            Member { id: id.clone(), username: person.username, display_name: person.display_name }
        })
        .collect()
}

/// The members of a group as asked: me first, then unique ids; new ones
/// must be accepted contacts, those already in `keep` may stay.
fn group_members(state: &State, keep: &[String], asked: &[String]) -> Result<Vec<String>, Failure> {
    let mut out = vec![state.me.id.clone()];
    for id in asked.iter().map(|m| m.trim()) {
        if out.iter().any(|m| m == id) {
            continue;
        }
        if !valid_user_id(id) || !(state.accepted(id) || keep.iter().any(|k| k == id)) {
            return Err(BAD);
        }
        out.push(id.to_owned());
    }
    if out.len() > MAX_MEMBERS {
        return Err(BAD);
    }
    Ok(out)
}

fn clean_title(title: &str) -> Result<String, Failure> {
    let title: String = title.trim().chars().take(100).collect();
    if title.is_empty() {
        return Err(BAD);
    }
    Ok(title)
}

fn new_group(user: &User, group: Group) -> Answer {
    let state = load(user)?;
    let title = clean_title(&group.title)?;
    let members = group_members(&state, &[], &group.members)?;
    let id = format!("g{}", random_id());
    let described = describe(&state, None, &members);
    deliver(user, &members, Body::RoomInvite { room: id.clone(), title: title.clone(), members: described.clone() })?;
    let room = Room {
        id: id.clone(),
        peer: None,
        title,
        owner: user.id.clone(),
        members,
        names: described
            .into_iter()
            .map(|m| (m.id, Person { username: m.username, display_name: m.display_name }))
            .collect(),
        last_at: Some(Utc::now()),
        last_text: String::new(),
        unread: 0,
        left: false,
    };
    commit(user, |state| state.rooms.insert(id.clone(), room.clone()))?;
    Ok(json!({ "id": id }))
}

fn change_group(user: &User, id: &str, group: Group) -> Answer {
    let state = load(user)?;
    let room = state.rooms.get(id).filter(|r| r.peer.is_none() && !r.left).ok_or(NOT_FOUND)?;
    if room.owner != user.id {
        return Err(FORBIDDEN);
    }
    let title = clean_title(&group.title)?;
    let before = room.members.clone();
    let members = group_members(&state, &before, &group.members)?;
    let described = describe(&state, Some(room), &members);
    // Everyone before and after hears of it: those taken out learn they left.
    let mut everyone = before.clone();
    everyone.extend(members.iter().filter(|m| !before.contains(m)).cloned());
    deliver(
        user,
        &everyone,
        Body::RoomUpdate { room: id.to_owned(), title: title.clone(), members: described.clone() },
    )?;
    let added: Vec<String> = members.iter().filter(|m| !before.contains(m)).cloned().collect();
    if !added.is_empty() {
        deliver(
            user,
            &added,
            Body::RoomInvite { room: id.to_owned(), title: title.clone(), members: described.clone() },
        )?;
    }
    let removed: Vec<String> = before.iter().filter(|m| !members.contains(m)).cloned().collect();
    remove_from_call(id, &removed);

    commit(user, |state| {
        if let Some(room) = state.rooms.get_mut(id) {
            room.title = title.clone();
            room.members = members.clone();
            for member in &described {
                room.names.entry(member.id.clone()).or_insert_with(|| Person {
                    username: member.username.clone(),
                    display_name: member.display_name.clone(),
                });
            }
        }
    })?;
    Ok(json!({}))
}

/// Takes people out of the chat's call, if one is going on. Best effort: the
/// short join tokens keep them out afterwards anyway.
fn remove_from_call(room: &str, ids: &[String]) {
    let Some(config) = livekit::Config::from_settings(&settings_json()) else { return };
    for id in ids {
        let Some((url, headers, body)) = livekit::remove_request(&config, room, id, Utc::now().timestamp()) else {
            return;
        };
        match http::fetch(&http::Request { method: "POST".into(), url, headers, body }) {
            // 404: not in the call, or no call.
            Ok(answer) if answer.status < 300 || answer.status == 404 => {}
            Ok(answer) => log::warn(&format!("talk: LiveKit RemoveParticipant answered {}", answer.status)),
            Err(e) => log::warn(&format!("talk: LiveKit RemoveParticipant: {e:?}")),
        }
    }
}

fn leave(user: &User, id: &str) -> Answer {
    let state = load(user)?;
    let room = state.rooms.get(id).filter(|r| r.peer.is_none() && !r.left).ok_or(NOT_FOUND)?;
    let others: Vec<String> = room.members.iter().filter(|m| **m != user.id).cloned().collect();
    deliver(user, &others, Body::RoomLeave { room: id.to_owned() })?;
    commit(user, |state| {
        if let Some(room) = state.rooms.get_mut(id) {
            room.members.retain(|m| *m != user.id);
            room.left = true;
        }
    })?;
    Ok(json!({}))
}

/// A room I may write in, and who else gets what I write.
fn writable<'a>(state: &'a State, user: &User, id: &str) -> Result<(&'a Room, Vec<String>), Failure> {
    let room = state.rooms.get(id).ok_or(NOT_FOUND)?;
    let peer_ok = room.peer.as_ref().is_none_or(|peer| state.accepted(peer));
    if room.left || !room.members.contains(&user.id) || !peer_ok {
        return Err(FORBIDDEN);
    }
    let others = room.members.iter().filter(|m| **m != user.id).cloned().collect();
    Ok((room, others))
}

fn messages(user: &User, id: &str, before: &str) -> Answer {
    let state = load(user)?;
    let room = state.rooms.get(id).ok_or(NOT_FOUND)?;
    let (messages, older) = store::recent(&room.id, before, 100).map_err(fail)?;
    Ok(json!({ "messages": messages, "older": older }))
}

fn send(user: &User, id: &str, text: &str) -> Answer {
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(BAD);
    }
    let state = load(user)?;
    let (room, others) = writable(&state, user, id)?;
    let start = Utc::now();
    let mut done = Vec::new();
    for (i, part) in split_text(text).into_iter().enumerate() {
        let envelope =
            envelope(Body::Msg { room: room.id.clone(), text: part.clone() }, start + Duration::milliseconds(i as i64));
        sent(delivery::send(user, &others, &envelope))?;
        let message = Message {
            id: envelope.id.clone(),
            from: user.id.clone(),
            at: envelope.sent_at,
            text: part,
            edited: false,
            deleted: false,
        };
        store::append(&room.id, message.clone()).map_err(fail)?;
        done.push(message);
    }
    if let Some(last) = done.last() {
        let (at, text): (_, String) = (last.at, last.text.chars().take(100).collect());
        commit(user, |state| {
            if let Some(room) = state.rooms.get_mut(id) {
                if room.last_at.is_none_or(|l| l <= at) {
                    room.last_at = Some(at);
                    room.last_text = text.clone();
                }
            }
        })?;
    }
    Ok(json!({ "messages": done }))
}

fn edit(user: &User, id: &str, change: Change) -> Answer {
    let text = change.text.trim_end().to_owned();
    if text.trim().is_empty() || split_text(&text).len() > 1 {
        return Err(BAD);
    }
    let state = load(user)?;
    let (room, others) = writable(&state, user, id)?;
    let room_id = room.id.clone();
    deliver(
        user,
        &others,
        Body::MsgEdit { room: room_id.clone(), msg: change.msg.clone(), at: change.at, text: text.clone() },
    )?;
    store::edit(&room_id, &day_of(&change.at), &change.msg, &user.id, &text).map_err(fail)?;
    Ok(json!({}))
}

fn delete(user: &User, id: &str, change: Change) -> Answer {
    let state = load(user)?;
    let (room, others) = writable(&state, user, id)?;
    let room_id = room.id.clone();
    deliver(user, &others, Body::MsgDelete { room: room_id.clone(), msg: change.msg.clone(), at: change.at })?;
    store::delete(&room_id, &day_of(&change.at), &change.msg, &user.id).map_err(fail)?;
    Ok(json!({}))
}

fn read(user: &User, id: &str) -> Answer {
    let waiting = commit(user, |state| {
        let found = state.rooms.get_mut(id).map(|room| room.unread = 0).is_some();
        found.then(|| state.waiting())
    })?;
    delivery::badge(waiting.ok_or(NOT_FOUND)?);
    Ok(json!({}))
}

fn call(user: &User, id: &str) -> Answer {
    let config = livekit::Config::from_settings(&settings_json()).ok_or((503, "CALLS_OFF"))?;
    let state = load(user)?;
    let (room, _) = writable(&state, user, id)?;
    let name = Person { username: user.username.clone(), display_name: user.display_name.clone() }.label();
    let token = livekit::join_token(&config, &room.id, &user.id, &name, Utc::now().timestamp());
    Ok(json!({ "url": config.url, "token": token, "room": livekit::room_name(&room.id) }))
}
