//! What Talk keeps and sends. Plain data, no host calls: tested natively.
//!
//! People are known by their account **id**: a username can be taken by
//! someone else after the account is deleted. The username is shown, never
//! trusted; the display name is shown only next to the username (anyone may
//! call themselves "Administrator").
//!
//! In each user's AppData (`.appdata/rc-talk/`):
//!
//! | path                            | what                                              |
//! |---------------------------------|---------------------------------------------------|
//! | `state.json`                    | [`Book`] (contacts by id, my requests by username) and `rooms` ([`Rooms`]: chats, members, unread) |
//! | `rooms/<id>/<YYYY-MM-DD>.jsonl` | [`Message`]s of a chat, one file per day (UTC)     |
//!
//! One file for the book and the rooms: a change to both is one
//! `write-if-match` (Talk 0.1 had `contacts.json` and `rooms.json`).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of [`Envelope`]; a recipient drops envelopes of newer versions.
pub const ENVELOPE_VERSION: u8 = 1;

/// The most an envelope may take, serialized: `data` of a job from
/// `jobs.add-for` (64 KiB).
pub const MAX_ENVELOPE: usize = 64 * 1024;

/// The most text one message carries, in JSON bytes; longer text is sent as
/// several messages. Well under [`MAX_ENVELOPE`], and a size people read.
pub const MAX_TEXT: usize = 16 * 1024;

/// Who sent an envelope. Given by the delivery, not by the envelope: with
/// `jobs.add-for` the server sets it, and a sender can't pretend to be another.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Sender {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

/// How to show someone.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Person {
    pub username: String,
    pub display_name: String,
}

impl From<&Sender> for Person {
    fn from(sender: &Sender) -> Person {
        Person { username: sender.username.clone(), display_name: sender.display_name.clone() }
    }
}

impl Person {
    /// "Alice (alice)", or just "alice": never the display name alone.
    pub fn label(&self) -> String {
        let display = self.display_name.trim();
        if display.is_empty() || display == self.username {
            self.username.clone()
        } else {
            format!("{display} ({})", self.username)
        }
    }
}

/// A member as the group's owner describes it. The names are the owner's
/// word until the member writes (then the server's [`Sender`] replaces them).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

/// One thing one user sends to others.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Envelope {
    pub v: u8,
    /// Unique: the job key with `jobs.add-for`, so an edit never replaces the
    /// message it edits while both wait.
    pub id: String,
    /// The sender's clock: orders a sender's own messages, nothing more.
    pub sent_at: DateTime<Utc>,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// "Let's talk", sent to a username: the only envelope taken from someone
    /// who isn't a contact.
    ContactRequest,
    /// The recipient accepted a request of ours.
    ContactAccept,
    /// A group chat the sender owns, with us among its members.
    RoomInvite {
        room: String,
        title: String,
        members: Vec<Member>,
    },
    /// The owner changed a group's title or members.
    RoomUpdate {
        room: String,
        title: String,
        members: Vec<Member>,
    },
    /// The sender left the group.
    RoomLeave {
        room: String,
    },
    Msg {
        room: String,
        text: String,
    },
    /// `at` is when the message was sent: it names the file of its day.
    MsgEdit {
        room: String,
        msg: String,
        at: DateTime<Utc>,
        text: String,
    },
    MsgDelete {
        room: String,
        msg: String,
        at: DateTime<Utc>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContactState {
    /// They asked, we haven't answered.
    Incoming,
    Accepted,
    /// Nothing of theirs is taken.
    Blocked,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Contact {
    pub state: ContactState,
    pub username: String,
    pub display_name: String,
    pub since: DateTime<Utc>,
}

impl Contact {
    pub fn person(&self) -> Person {
        Person { username: self.username.clone(), display_name: self.display_name.clone() }
    }
}

/// Contacts by account id, and the usernames I asked (their id comes with the answer).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Book {
    pub contacts: BTreeMap<String, Contact>,
    pub asked: BTreeMap<String, DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Room {
    pub id: String,
    /// The other person's id in a direct chat; `None` for a group.
    pub peer: Option<String>,
    pub title: String,
    /// Who may change the group (an id); the peer in a direct chat.
    pub owner: String,
    /// Account ids, mine included.
    pub members: Vec<String>,
    /// How to show the members, by id.
    pub names: BTreeMap<String, Person>,
    pub last_at: Option<DateTime<Utc>>,
    pub last_text: String,
    pub unread: u32,
    /// I'm no longer in the group: kept for reading, nothing new arrives.
    #[serde(default)]
    pub left: bool,
}

/// By room id.
pub type Rooms = BTreeMap<String, Room>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub id: String,
    /// The author's account id.
    pub from: String,
    pub at: DateTime<Utc>,
    pub text: String,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub deleted: bool,
}

/// The id of the direct chat of two accounts: the same on both sides.
pub fn direct_room_id(a: &str, b: &str) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let hash = Sha256::digest(format!("{first}\n{second}").as_bytes());
    let hex: String = hash[..12].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("d{hex}")
}

/// A room id we made or got: letters and digits only, so it is safe in paths.
pub fn valid_room_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// An account id (a UUID) we may put in a storage path.
pub fn valid_user_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A username we may put in a storage path. The server's own rules are
/// stricter; this only keeps paths safe.
pub fn valid_username(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && name != "."
        && name != ".."
}

/// Splits text into pieces that take at most [`MAX_TEXT`] bytes in JSON
/// (quotes and control characters grow when escaped), on character boundaries.
pub fn split_text(text: &str) -> Vec<String> {
    fn json_len(ch: char) -> usize {
        match ch {
            '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
            ch if (ch as u32) < 0x20 => 6,
            ch => ch.len_utf8(),
        }
    }
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut size = 0;
    for ch in text.chars() {
        if size + json_len(ch) > MAX_TEXT {
            parts.push(std::mem::take(&mut part));
            size = 0;
        }
        size += json_len(ch);
        part.push(ch);
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// The day file a message of `at` belongs to.
pub fn day_of(at: &DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_room_is_the_same_both_ways() {
        assert_eq!(direct_room_id("id-a", "id-b"), direct_room_id("id-b", "id-a"));
        assert_ne!(direct_room_id("id-a", "id-b"), direct_room_id("id-a", "id-c"));
        assert!(valid_room_id(&direct_room_id("id-a", "id-b")));
    }

    #[test]
    fn long_text_splits_on_characters() {
        let text = "я".repeat(MAX_TEXT); // two bytes each
        let parts = split_text(&text);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| p.len() <= MAX_TEXT));
        assert_eq!(parts.concat(), text);
    }

    #[test]
    fn biggest_messages_fit_a_job() {
        for text in ["\u{1F600}".repeat(10_000), "\"".repeat(20_000), "\u{1}".repeat(20_000)] {
            for part in split_text(&text) {
                let envelope = Envelope {
                    v: ENVELOPE_VERSION,
                    id: "0".repeat(32),
                    sent_at: Utc::now(),
                    body: Body::Msg { room: "0".repeat(64), text: part },
                };
                assert!(serde_json::to_vec(&envelope).unwrap().len() <= MAX_ENVELOPE);
            }
        }
    }

    #[test]
    fn names_stay_inside_paths() {
        assert!(valid_username("alice.smith"));
        assert!(valid_user_id("0b6f3c1e-5f0a-4a43-9d6e-2f4f3d1a7c55"));
        for bad in ["", ".", "..", "a/b", "a\\b", "a b"] {
            assert!(!valid_username(bad), "{bad:?}");
            assert!(!valid_user_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn display_name_never_shown_alone() {
        let impostor = Person { username: "mallory".into(), display_name: "Administrator".into() };
        assert_eq!(impostor.label(), "Administrator (mallory)");
        let plain = Person { username: "bob".into(), display_name: "bob".into() };
        assert_eq!(plain.label(), "bob");
    }
}
