//! The page's JSON API under `/app/rc-talk/api/`. People are named by account
//! id (`id`), except a new contact request, which can only name a username.
//!
//! | request                              | body                       | what                                  |
//! |--------------------------------------|----------------------------|---------------------------------------|
//! | `GET  /state`                        |                            | me, contacts, asked, rooms, what works |
//! | `POST /sync`                         |                            | take what waits for me → `{taken}`    |
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

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::delivery::{self, Mode};
use crate::inbox::{State, MAX_MEMBERS};
use crate::model::*;
use crate::rustcloud::app::{http, log, settings};
use crate::{livekit, random_id, Response, User};

type Failure = (u16, &'static str);
type Answer = Result<Value, Failure>;

const BAD: Failure = (400, "BAD_REQUEST");
const NOT_FOUND: Failure = (404, "NOT_FOUND");
const FORBIDDEN: Failure = (403, "FORBIDDEN");
const DELIVERY_OFF: Failure = (503, "DELIVERY_OFF");

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
        ("POST", ["sync"]) => {
            let taken = delivery::take_incoming(user).map_err(fail)?;
            Ok(json!({ "taken": taken }))
        }
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
    crate::store::load(delivery::sender(user)).map_err(fail)
}

fn save(state: &State) -> Result<(), Failure> {
    crate::store::save(state).map_err(fail)
}

fn envelope(body: Body, sent_at: DateTime<Utc>) -> Envelope {
    Envelope { v: ENVELOPE_VERSION, id: random_id(), sent_at, body }
}

fn require_delivery() -> Result<(), Failure> {
    match delivery::mode() {
        Mode::Off => Err(DELIVERY_OFF),
        Mode::Storage => Ok(()),
    }
}

/// Sends a new envelope to account ids.
fn deliver(user: &User, to: &[String], body: Body) -> Result<(), Failure> {
    delivery::send(user, to, &envelope(body, Utc::now())).map_err(fail)
}

fn settings_json() -> Value {
    serde_json::from_str(&settings::values()).unwrap_or_default()
}

fn state(user: &User) -> Answer {
    let state = load(user)?;
    Ok(json!({
        "me": { "id": user.id, "username": user.username, "display_name": user.display_name },
        "contacts": state.book.contacts,
        "asked": state.book.asked,
        "rooms": state.rooms,
        "delivery": delivery::mode(),
        "calls": livekit::Config::from_settings(&settings_json()).is_some(),
    }))
}

fn ask(user: &User, username: String) -> Answer {
    let username = username.trim().to_owned();
    if !valid_username(&username) || username == user.username {
        return Err(BAD);
    }
    let mut state = load(user)?;
    // Someone with this username already in the book: answer by their id.
    let known = state.book.contacts.iter().find(|(_, c)| c.username == username).map(|(id, c)| (id.clone(), c.state));
    match known {
        Some((id, ContactState::Incoming)) => return accept(user, &id),
        Some((_, contact_state)) => return Ok(json!({ "state": contact_state })),
        None => {}
    }
    require_delivery()?;
    delivery::ask(user, &username, &envelope(Body::ContactRequest, Utc::now())).map_err(fail)?;
    state.book.asked.insert(username, Utc::now());
    save(&state)?;
    Ok(json!({ "state": "asked" }))
}

fn accept(user: &User, id: &str) -> Answer {
    let mut state = load(user)?;
    let contact = state.book.contacts.get(id).filter(|c| c.state == ContactState::Incoming).ok_or(NOT_FOUND)?;
    let person = contact.person();
    require_delivery()?;
    deliver(user, &[id.to_owned()], Body::ContactAccept)?;
    let contact = state.book.contacts.get_mut(id).expect("checked above");
    contact.state = ContactState::Accepted;
    contact.since = Utc::now();
    state.direct_room(id, person);
    save(&state)?;
    Ok(json!({ "state": ContactState::Accepted }))
}

fn set_contact(user: &User, id: &str, to: Option<ContactState>) -> Answer {
    let mut state = load(user)?;
    match to {
        // Only someone already in the book: their names came from the server.
        Some(next) => state.book.contacts.get_mut(id).ok_or(NOT_FOUND)?.state = next,
        None => {
            state.book.contacts.remove(id);
        }
    }
    save(&state)?;
    Ok(json!({}))
}

fn cancel_ask(user: &User, username: &str) -> Answer {
    let mut state = load(user)?;
    state.book.asked.remove(username.trim());
    save(&state)?;
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
    let mut state = load(user)?;
    let title = clean_title(&group.title)?;
    let members = group_members(&state, &[], &group.members)?;
    require_delivery()?;
    let id = format!("g{}", random_id());
    let described = describe(&state, None, &members);
    deliver(user, &members, Body::RoomInvite { room: id.clone(), title: title.clone(), members: described.clone() })?;
    state.rooms.insert(
        id.clone(),
        Room {
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
        },
    );
    save(&state)?;
    Ok(json!({ "id": id }))
}

fn change_group(user: &User, id: &str, group: Group) -> Answer {
    let mut state = load(user)?;
    let room = state.rooms.get(id).filter(|r| r.peer.is_none() && !r.left).ok_or(NOT_FOUND)?;
    if room.owner != user.id {
        return Err(FORBIDDEN);
    }
    let title = clean_title(&group.title)?;
    let before = room.members.clone();
    let members = group_members(&state, &before, &group.members)?;
    let described = describe(&state, Some(room), &members);
    require_delivery()?;
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

    let room = state.rooms.get_mut(id).expect("checked above");
    room.title = title;
    room.members = members;
    for member in described {
        room.names.entry(member.id).or_insert(Person { username: member.username, display_name: member.display_name });
    }
    save(&state)?;
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
    let mut state = load(user)?;
    let room = state.rooms.get_mut(id).filter(|r| r.peer.is_none() && !r.left).ok_or(NOT_FOUND)?;
    let others: Vec<String> = room.members.iter().filter(|m| **m != user.id).cloned().collect();
    require_delivery()?;
    deliver(user, &others, Body::RoomLeave { room: id.to_owned() })?;
    let room = state.rooms.get_mut(id).expect("checked above");
    room.members.retain(|m| *m != user.id);
    room.left = true;
    save(&state)?;
    Ok(json!({}))
}

/// A room I may write in, and who else gets what I write.
fn writable<'a>(state: &'a mut State, user: &User, id: &str) -> Result<(&'a mut Room, Vec<String>), Failure> {
    let peer_ok = match state.rooms.get(id).and_then(|r| r.peer.clone()) {
        Some(peer) => state.accepted(&peer),
        None => true,
    };
    let room = state.rooms.get_mut(id).ok_or(NOT_FOUND)?;
    if room.left || !room.members.contains(&user.id) || !peer_ok {
        return Err(FORBIDDEN);
    }
    let others = room.members.iter().filter(|m| **m != user.id).cloned().collect();
    Ok((room, others))
}

fn messages(user: &User, id: &str, before: &str) -> Answer {
    let state = load(user)?;
    let room = state.rooms.get(id).ok_or(NOT_FOUND)?;
    let (messages, older) = crate::store::recent(&room.id, before, 100).map_err(fail)?;
    Ok(json!({ "messages": messages, "older": older }))
}

fn send(user: &User, id: &str, text: &str) -> Answer {
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err(BAD);
    }
    require_delivery()?;
    let mut state = load(user)?;
    let (room, others) = writable(&mut state, user, id)?;
    let start = Utc::now();
    let mut sent = Vec::new();
    for (i, part) in split_text(text).into_iter().enumerate() {
        let envelope =
            envelope(Body::Msg { room: room.id.clone(), text: part.clone() }, start + Duration::milliseconds(i as i64));
        delivery::send(user, &others, &envelope).map_err(fail)?;
        let message = Message {
            id: envelope.id.clone(),
            from: user.id.clone(),
            at: envelope.sent_at,
            text: part,
            edited: false,
            deleted: false,
        };
        crate::store::append(&room.id, message.clone()).map_err(fail)?;
        room.last_at = Some(message.at);
        room.last_text = message.text.chars().take(100).collect();
        sent.push(message);
    }
    save(&state)?;
    Ok(json!({ "messages": sent }))
}

fn edit(user: &User, id: &str, change: Change) -> Answer {
    let text = change.text.trim_end().to_owned();
    if text.trim().is_empty() || split_text(&text).len() > 1 {
        return Err(BAD);
    }
    require_delivery()?;
    let mut state = load(user)?;
    let (room, others) = writable(&mut state, user, id)?;
    let room_id = room.id.clone();
    deliver(
        user,
        &others,
        Body::MsgEdit { room: room_id.clone(), msg: change.msg.clone(), at: change.at, text: text.clone() },
    )?;
    crate::store::edit(&room_id, &day_of(&change.at), &change.msg, &user.id, &text).map_err(fail)?;
    Ok(json!({}))
}

fn delete(user: &User, id: &str, change: Change) -> Answer {
    require_delivery()?;
    let mut state = load(user)?;
    let (room, others) = writable(&mut state, user, id)?;
    let room_id = room.id.clone();
    deliver(user, &others, Body::MsgDelete { room: room_id.clone(), msg: change.msg.clone(), at: change.at })?;
    crate::store::delete(&room_id, &day_of(&change.at), &change.msg, &user.id).map_err(fail)?;
    Ok(json!({}))
}

fn read(user: &User, id: &str) -> Answer {
    let mut state = load(user)?;
    let room = state.rooms.get_mut(id).ok_or(NOT_FOUND)?;
    if room.unread > 0 {
        room.unread = 0;
        save(&state)?;
    }
    Ok(json!({}))
}

fn call(user: &User, id: &str) -> Answer {
    let config = livekit::Config::from_settings(&settings_json()).ok_or((503, "CALLS_OFF"))?;
    let mut state = load(user)?;
    let (room, _) = writable(&mut state, user, id)?;
    let name = Person { username: user.username.clone(), display_name: user.display_name.clone() }.label();
    let token = livekit::join_token(&config, &room.id, &user.id, &name, Utc::now().timestamp());
    Ok(json!({ "url": config.url, "token": token, "room": livekit::room_name(&room.id) }))
}
