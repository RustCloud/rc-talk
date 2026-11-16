//! What a recipient does with an envelope: who may send what, and what
//! changes. Pure: the caller loads the state, applies, then saves the state
//! and carries out the [`Effect`]s on the day files.
//!
//! Everyone is matched by account id ([`Sender::id`]); names are only shown.
//!
//! Rules:
//! - from someone who isn't an accepted contact only `contact_request` is taken
//!   (and nothing is answered automatically: an answer would tell the sender
//!   that the username exists);
//! - nothing from a blocked contact;
//! - a group is created only by an invite from its owner, an accepted contact;
//!   only the owner changes it; a member leaves only by themself; messages
//!   come only from its members;
//! - a message is edited or deleted only by its author (checked by the store,
//!   which has the message).

use crate::model::*;

/// The most members a group has (as `jobs.add-for` takes at most 50 recipients).
pub const MAX_MEMBERS: usize = 50;

pub struct State {
    pub me: Sender,
    pub book: Book,
    pub rooms: Rooms,
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    Append { room: String, message: Message },
    Edit { room: String, day: String, msg: String, from: String, text: String },
    Delete { room: String, day: String, msg: String, from: String },
}

impl State {
    pub fn accepted(&self, id: &str) -> bool {
        self.book.contacts.get(id).is_some_and(|c| c.state == ContactState::Accepted)
    }

    /// The number by Talk in the side panel: unread messages and requests.
    pub fn waiting(&self) -> u32 {
        let requests = self.book.contacts.values().filter(|c| c.state == ContactState::Incoming).count() as u32;
        self.rooms.values().filter(|r| !r.left).map(|r| r.unread).sum::<u32>() + requests
    }

    /// The direct chat with `peer`, made if there is none.
    pub fn direct_room(&mut self, peer: &str, person: Person) -> &mut Room {
        let id = direct_room_id(&self.me.id, peer);
        let me = self.me.clone();
        let room = self.rooms.entry(id.clone()).or_insert_with(|| Room {
            id,
            peer: Some(peer.to_owned()),
            title: String::new(),
            owner: peer.to_owned(),
            members: vec![me.id.clone(), peer.to_owned()],
            names: [(me.id.clone(), Person::from(&me))].into(),
            last_at: None,
            last_text: String::new(),
            unread: 0,
            left: false,
        });
        room.title = person.label();
        room.names.insert(peer.to_owned(), person);
        room
    }

    /// The room `id` if `sender` may write in it now.
    fn room_for(&mut self, id: &str, sender: &Sender) -> Option<&mut Room> {
        if *id == direct_room_id(&self.me.id, &sender.id) {
            if !self.accepted(&sender.id) {
                return None;
            }
            return Some(self.direct_room(&sender.id, Person::from(sender)));
        }
        let room = self
            .rooms
            .get_mut(id)
            .filter(|room| room.peer.is_none() && !room.left && room.members.contains(&sender.id))?;
        // The server's word on who wrote replaces the owner's.
        room.names.insert(sender.id.clone(), Person::from(sender));
        Some(room)
    }

    /// Keeps a contact's names as the server gives them.
    fn refresh_contact(&mut self, sender: &Sender) {
        if let Some(contact) = self.book.contacts.get_mut(&sender.id) {
            contact.username = sender.username.clone();
            contact.display_name = sender.display_name.clone();
        }
    }
}

/// Members from an invite or update: valid ids, unique, not too many.
fn members_of(list: &[Member]) -> Option<Vec<Member>> {
    let mut out: Vec<Member> = Vec::new();
    for member in list {
        if !valid_user_id(&member.id) {
            return None;
        }
        if !out.iter().any(|m| m.id == member.id) {
            out.push(member.clone());
        }
    }
    (out.len() <= MAX_MEMBERS).then_some(out)
}

/// Sets a group's members; names we already had (the server's word) stay.
fn set_members(room: &mut Room, members: &[Member]) {
    room.members = members.iter().map(|m| m.id.clone()).collect();
    for member in members {
        room.names
            .entry(member.id.clone())
            .or_insert_with(|| Person { username: member.username.clone(), display_name: member.display_name.clone() });
    }
}

pub fn apply(state: &mut State, from: &Sender, envelope: &Envelope, now: chrono::DateTime<chrono::Utc>) -> Vec<Effect> {
    if envelope.v > ENVELOPE_VERSION || from.id == state.me.id || !valid_user_id(&from.id) {
        return Vec::new();
    }
    match state.book.contacts.get(&from.id).map(|c| c.state) {
        Some(ContactState::Blocked) => return Vec::new(),
        Some(_) => state.refresh_contact(from),
        None => {}
    }
    match &envelope.body {
        Body::ContactRequest => {
            if state.book.contacts.contains_key(&from.id) {
                return Vec::new();
            }
            // Both asked: that's agreement on both sides.
            let both = state.book.asked.remove(&from.username).is_some();
            let contact_state = if both { ContactState::Accepted } else { ContactState::Incoming };
            state.book.contacts.insert(
                from.id.clone(),
                Contact {
                    state: contact_state,
                    username: from.username.clone(),
                    display_name: from.display_name.clone(),
                    since: now,
                },
            );
            if both {
                state.direct_room(&from.id, Person::from(from));
            }
            Vec::new()
        }
        Body::ContactAccept => {
            if state.book.contacts.contains_key(&from.id) || state.book.asked.remove(&from.username).is_none() {
                return Vec::new();
            }
            state.book.contacts.insert(
                from.id.clone(),
                Contact {
                    state: ContactState::Accepted,
                    username: from.username.clone(),
                    display_name: from.display_name.clone(),
                    since: now,
                },
            );
            state.direct_room(&from.id, Person::from(from));
            Vec::new()
        }
        Body::RoomInvite { room, title, members } => {
            let Some(members) = members_of(members) else { return Vec::new() };
            let me = state.me.id.clone();
            let rejoin = state.rooms.get(room).is_some_and(|r| r.peer.is_none() && r.left && r.owner == from.id);
            if !valid_room_id(room)
                || !room.starts_with('g')
                || !state.accepted(&from.id)
                || !members.iter().any(|m| m.id == me)
                || (state.rooms.contains_key(room) && !rejoin)
            {
                return Vec::new();
            }
            let entry = state.rooms.entry(room.clone()).or_insert_with(|| Room {
                id: room.clone(),
                peer: None,
                title: String::new(),
                owner: from.id.clone(),
                members: Vec::new(),
                names: Default::default(),
                last_at: Some(now),
                last_text: String::new(),
                unread: 0,
                left: false,
            });
            entry.title = title.chars().take(100).collect();
            entry.left = false;
            entry.names.insert(from.id.clone(), Person::from(from));
            entry.names.insert(me, Person::from(&state.me));
            set_members(entry, &members);
            Vec::new()
        }
        Body::RoomUpdate { room, title, members } => {
            let Some(members) = members_of(members) else { return Vec::new() };
            let me = state.me.id.clone();
            if let Some(room) = state.rooms.get_mut(room).filter(|r| r.peer.is_none() && r.owner == from.id && !r.left)
            {
                room.title = title.chars().take(100).collect();
                set_members(room, &members);
                room.left = !room.members.contains(&me);
            }
            Vec::new()
        }
        Body::RoomLeave { room } => {
            if let Some(room) = state.rooms.get_mut(room).filter(|r| r.peer.is_none() && !r.left) {
                // Only the sender themself leaves: nobody else is taken out this way.
                room.members.retain(|m| *m != from.id);
            }
            Vec::new()
        }
        Body::Msg { room, text } => {
            let Some(room) = state.room_for(room, from) else { return Vec::new() };
            let message = Message {
                id: envelope.id.clone(),
                from: from.id.clone(),
                at: envelope.sent_at,
                text: text.clone(),
                edited: false,
                deleted: false,
            };
            room.unread += 1;
            if room.last_at.is_none_or(|last| last <= envelope.sent_at) {
                room.last_at = Some(envelope.sent_at);
                room.last_text = text.chars().take(100).collect();
            }
            vec![Effect::Append { room: room.id.clone(), message }]
        }
        Body::MsgEdit { room, msg, at, text } => match state.room_for(room, from) {
            Some(room) => vec![Effect::Edit {
                room: room.id.clone(),
                day: day_of(at),
                msg: msg.clone(),
                from: from.id.clone(),
                text: text.clone(),
            }],
            None => Vec::new(),
        },
        Body::MsgDelete { room, msg, at } => match state.room_for(room, from) {
            Some(room) => {
                vec![Effect::Delete { room: room.id.clone(), day: day_of(at), msg: msg.clone(), from: from.id.clone() }]
            }
            None => Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sender(name: &str) -> Sender {
        Sender { id: format!("id-{name}"), username: name.into(), display_name: name.to_uppercase() }
    }

    fn state(me: &str) -> State {
        State { me: sender(me), book: Book::default(), rooms: Rooms::new() }
    }

    fn member(name: &str) -> Member {
        let s = sender(name);
        Member { id: s.id, username: s.username, display_name: s.display_name }
    }

    fn envelope(body: Body) -> Envelope {
        Envelope { v: ENVELOPE_VERSION, id: "m1".into(), sent_at: Utc::now(), body }
    }

    fn befriend(s: &mut State, name: &str) {
        let who = sender(name);
        s.book.contacts.insert(
            who.id,
            Contact {
                state: ContactState::Accepted,
                username: who.username,
                display_name: who.display_name,
                since: Utc::now(),
            },
        );
    }

    #[test]
    fn strangers_only_ask() {
        let mut bob = state("bob");
        let room = direct_room_id("id-bob", "id-alice");
        let msg = envelope(Body::Msg { room: room.clone(), text: "hi".into() });
        assert!(apply(&mut bob, &sender("alice"), &msg, Utc::now()).is_empty());
        assert!(bob.rooms.is_empty());

        apply(&mut bob, &sender("alice"), &envelope(Body::ContactRequest), Utc::now());
        assert_eq!(bob.book.contacts["id-alice"].state, ContactState::Incoming);
        // Still not a contact: messages are dropped until bob accepts.
        assert!(apply(&mut bob, &sender("alice"), &msg, Utc::now()).is_empty());
    }

    #[test]
    fn accepted_contact_writes_in_the_direct_chat() {
        let mut alice = state("alice");
        alice.book.asked.insert("bob".into(), Utc::now());
        apply(&mut alice, &sender("bob"), &envelope(Body::ContactAccept), Utc::now());
        assert_eq!(alice.book.contacts["id-bob"].state, ContactState::Accepted);
        assert!(alice.book.asked.is_empty());

        let room = direct_room_id("id-alice", "id-bob");
        let msg = Body::Msg { room: room.clone(), text: "hi".into() };
        let effects = apply(&mut alice, &sender("bob"), &envelope(msg), Utc::now());
        assert!(
            matches!(&effects[..], [Effect::Append { room: r, message }] if *r == room && message.from == "id-bob")
        );
        assert_eq!(alice.rooms[&room].unread, 1);
        assert_eq!(alice.rooms[&room].title, "BOB (bob)");
    }

    #[test]
    fn the_badge_counts_unread_and_requests() {
        let mut alice = state("alice");
        befriend(&mut alice, "bob");
        let room = direct_room_id("id-alice", "id-bob");
        for _ in 0..2 {
            let msg = Body::Msg { room: room.clone(), text: "hi".into() };
            apply(&mut alice, &sender("bob"), &envelope(msg), Utc::now());
        }
        apply(&mut alice, &sender("carol"), &envelope(Body::ContactRequest), Utc::now());
        assert_eq!(alice.waiting(), 3);
        alice.rooms.get_mut(&room).unwrap().left = true;
        assert_eq!(alice.waiting(), 1);
    }

    #[test]
    fn a_new_owner_of_a_username_is_someone_else() {
        // The old bob was a contact; his account was deleted and a new one took "bob".
        let mut alice = state("alice");
        befriend(&mut alice, "bob");
        let new_bob = Sender { id: "id-new".into(), username: "bob".into(), display_name: "Bob".into() };
        let old_room = direct_room_id("id-alice", "id-bob");
        let msg = Body::Msg { room: old_room, text: "it's me".into() };
        assert!(apply(&mut alice, &new_bob, &envelope(msg), Utc::now()).is_empty());
        apply(&mut alice, &new_bob, &envelope(Body::ContactRequest), Utc::now());
        assert_eq!(alice.book.contacts["id-new"].state, ContactState::Incoming);
        assert_eq!(alice.book.contacts["id-bob"].state, ContactState::Accepted);
    }

    #[test]
    fn accept_without_request_is_ignored() {
        let mut alice = state("alice");
        apply(&mut alice, &sender("mallory"), &envelope(Body::ContactAccept), Utc::now());
        assert!(alice.book.contacts.is_empty());
    }

    #[test]
    fn blocked_sender_is_ignored() {
        let mut bob = state("bob");
        let mallory = sender("mallory");
        bob.book.contacts.insert(
            mallory.id.clone(),
            Contact {
                state: ContactState::Blocked,
                username: "mallory".into(),
                display_name: "m".into(),
                since: Utc::now(),
            },
        );
        apply(&mut bob, &mallory, &envelope(Body::ContactRequest), Utc::now());
        assert_eq!(bob.book.contacts[&mallory.id].state, ContactState::Blocked);
    }

    #[test]
    fn only_the_owner_changes_a_group_and_only_members_write() {
        let mut bob = state("bob");
        befriend(&mut bob, "alice");
        befriend(&mut bob, "carol");
        let invite =
            Body::RoomInvite { room: "g1".into(), title: "Team".into(), members: vec![member("alice"), member("bob")] };
        apply(&mut bob, &sender("alice"), &envelope(invite), Utc::now());
        assert_eq!(bob.rooms["g1"].owner, "id-alice");

        let hijack =
            Body::RoomUpdate { room: "g1".into(), title: "Mine".into(), members: vec![member("bob"), member("carol")] };
        apply(&mut bob, &sender("carol"), &envelope(hijack), Utc::now());
        assert_eq!(bob.rooms["g1"].title, "Team");

        let msg = Body::Msg { room: "g1".into(), text: "hi".into() };
        assert!(apply(&mut bob, &sender("carol"), &envelope(msg.clone()), Utc::now()).is_empty());
        assert_eq!(apply(&mut bob, &sender("alice"), &envelope(msg), Utc::now()).len(), 1);

        let kick = Body::RoomUpdate { room: "g1".into(), title: "Team".into(), members: vec![member("alice")] };
        apply(&mut bob, &sender("alice"), &envelope(kick), Utc::now());
        assert!(bob.rooms["g1"].left);
    }

    #[test]
    fn owner_names_give_way_to_the_server() {
        let mut bob = state("bob");
        befriend(&mut bob, "alice");
        let lie = Member { id: "id-carol".into(), username: "admin".into(), display_name: "Administrator".into() };
        let invite = Body::RoomInvite {
            room: "g1".into(),
            title: "T".into(),
            members: vec![member("alice"), member("bob"), lie],
        };
        apply(&mut bob, &sender("alice"), &envelope(invite), Utc::now());
        assert_eq!(bob.rooms["g1"].names["id-carol"].username, "admin");
        apply(&mut bob, &sender("carol"), &envelope(Body::Msg { room: "g1".into(), text: "hi".into() }), Utc::now());
        assert_eq!(bob.rooms["g1"].names["id-carol"].username, "carol");
    }

    #[test]
    fn a_member_leaves_only_by_themself() {
        let mut bob = state("bob");
        befriend(&mut bob, "alice");
        let members = vec![member("alice"), member("bob"), member("carol")];
        apply(
            &mut bob,
            &sender("alice"),
            &envelope(Body::RoomInvite { room: "g1".into(), title: "T".into(), members }),
            Utc::now(),
        );
        apply(&mut bob, &sender("carol"), &envelope(Body::RoomLeave { room: "g1".into() }), Utc::now());
        assert_eq!(bob.rooms["g1"].members, vec!["id-alice", "id-bob"]);
        assert!(!bob.rooms["g1"].left);
    }

    #[test]
    fn invite_cannot_take_a_direct_chat_id() {
        let mut bob = state("bob");
        befriend(&mut bob, "alice");
        let room = direct_room_id("id-bob", "id-carol");
        let invite = Body::RoomInvite { room: room.clone(), title: "x".into(), members: vec![member("bob")] };
        apply(&mut bob, &sender("alice"), &envelope(invite), Utc::now());
        assert!(!bob.rooms.contains_key(&room));
    }
}
