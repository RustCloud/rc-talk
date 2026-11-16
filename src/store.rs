//! The user's AppData: `state.json` and day files of messages (layout in
//! [`crate::model`]).
//!
//! The page and a delivery job of one user may write at once (jobs of one
//! user never run together: `one_batch_per_user`). So every write is
//! `write-if-match` with the etag the file was read with; on `conflict` the
//! change is made again on what the other one wrote.

use serde::{Deserialize, Serialize};

use crate::inbox::{Effect, State};
use crate::model::*;
use crate::rustcloud::app::appdata;

pub type Result<T> = std::result::Result<T, String>;

const STATE: &str = "state.json";

/// How many times a change is made again before giving up.
const TRIES: usize = 8;

/// What `state.json` holds.
#[derive(Deserialize, Default)]
struct Saved {
    #[serde(flatten)]
    book: Book,
    #[serde(default)]
    rooms: Rooms,
}

#[derive(Serialize)]
struct SavedRef<'a> {
    #[serde(flatten)]
    book: &'a Book,
    rooms: &'a Rooms,
}

/// A file as read: its content (`None` if there is none) and the etag to
/// write it back with (`""`: write only if there is still none).
struct Tagged {
    bytes: Option<Vec<u8>>,
    etag: String,
}

fn read_tagged(path: &str) -> Result<Tagged> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    // The etag first, the content second: a write in between leaves an etag
    // older than the content, and writing back fails with a conflict.
    let etag = match appdata::list(dir) {
        Ok(entries) => entries.into_iter().find(|e| e.name == name && !e.is_dir).map(|e| e.etag),
        Err(appdata::Error::NotFound) => None,
        Err(e) => return Err(format!("{dir}: {e:?}")),
    };
    let Some(etag) = etag else { return Ok(Tagged { bytes: None, etag: String::new() }) };
    match appdata::read(path) {
        Ok(bytes) => Ok(Tagged { bytes: Some(bytes), etag }),
        // Deleted meanwhile: the old etag makes the write conflict.
        Err(appdata::Error::NotFound) => Ok(Tagged { bytes: None, etag }),
        Err(e) => Err(format!("{path}: {e:?}")),
    }
}

/// `false` when someone wrote since the file had `etag`.
fn write_tagged(path: &str, bytes: &[u8], etag: &str) -> Result<bool> {
    match appdata::write_if_match(path, bytes, etag) {
        Ok(_) => Ok(true),
        Err(appdata::Error::Conflict) => Ok(false),
        Err(e) => Err(format!("{path}: {e:?}")),
    }
}

/// Rewrites `path` with what `change` makes of its content (`None`: leave it
/// as it is), again if someone wrote in between.
fn modify(path: &str, mut change: impl FnMut(Option<&[u8]>) -> Option<Vec<u8>>) -> Result<()> {
    for _ in 0..TRIES {
        let file = read_tagged(path)?;
        let Some(bytes) = change(file.bytes.as_deref()) else { return Ok(()) };
        if write_tagged(path, &bytes, &file.etag)? {
            return Ok(());
        }
    }
    Err(format!("{path}: keeps changing"))
}

/// Talk 0.1 kept `contacts.json` and `rooms.json`: read until `state.json` is
/// written, then deleted.
fn legacy() -> Result<(Book, Rooms)> {
    fn read<T: for<'a> Deserialize<'a> + Default>(path: &str) -> Result<T> {
        match appdata::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}")),
            Err(appdata::Error::NotFound) => Ok(T::default()),
            Err(e) => Err(format!("{path}: {e:?}")),
        }
    }
    Ok((read("contacts.json")?, read("rooms.json")?))
}

fn parse_state(bytes: Option<&[u8]>) -> Result<(Book, Rooms)> {
    match bytes {
        Some(bytes) => {
            let saved: Saved = serde_json::from_slice(bytes).map_err(|e| format!("{STATE}: {e}"))?;
            Ok((saved.book, saved.rooms))
        }
        None => legacy(),
    }
}

/// The state as it is now, to look at.
pub fn load(me: Sender) -> Result<State> {
    let file = read_tagged(STATE)?;
    let (book, rooms) = parse_state(file.bytes.as_deref())?;
    Ok(State { me, book, rooms })
}

/// Loads the state, lets `change` work on it and saves it if it changed. If
/// someone wrote in between (a delivery while the page asked, or the other
/// way round), `change` runs again on what they wrote: it must do nothing but
/// change the state, and expect anything it checked before to have changed.
pub fn update<T>(me: &Sender, mut change: impl FnMut(&mut State) -> T) -> Result<T> {
    for _ in 0..TRIES {
        let file = read_tagged(STATE)?;
        let (book, rooms) = parse_state(file.bytes.as_deref())?;
        let mut state = State { me: me.clone(), book, rooms };
        let out = change(&mut state);
        let bytes = serde_json::to_vec(&SavedRef { book: &state.book, rooms: &state.rooms })
            .map_err(|e| format!("{STATE}: {e}"))?;
        if file.bytes.as_deref() == Some(bytes.as_slice()) {
            return Ok(out);
        }
        if write_tagged(STATE, &bytes, &file.etag)? {
            if file.etag.is_empty() {
                let _ = appdata::delete("contacts.json");
                let _ = appdata::delete("rooms.json");
            }
            return Ok(out);
        }
    }
    Err(format!("{STATE}: keeps changing"))
}

fn day_path(room: &str, day: &str) -> String {
    format!("rooms/{room}/{day}.jsonl")
}

fn parse_day(bytes: &[u8]) -> Vec<Message> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn day_bytes(messages: &[Message]) -> Vec<u8> {
    let mut out = String::new();
    for line in messages.iter().filter_map(|m| serde_json::to_string(m).ok()) {
        out.push_str(&line);
        out.push('\n');
    }
    out.into_bytes()
}

pub fn read_day(room: &str, day: &str) -> Result<Vec<Message>> {
    match appdata::read(&day_path(room, day)) {
        Ok(bytes) => Ok(parse_day(&bytes)),
        Err(appdata::Error::NotFound) => Ok(Vec::new()),
        Err(e) => Err(format!("{room}/{day}: {e:?}")),
    }
}

/// Adds a message to its day, ordered by time and then id (retried jobs may
/// run after later ones); a message already there (the same id delivered
/// twice) is left as it is.
pub fn append(room: &str, message: Message) -> Result<()> {
    modify(&day_path(room, &day_of(&message.at)), |bytes| {
        let mut messages = bytes.map(parse_day).unwrap_or_default();
        if messages.iter().any(|m| m.id == message.id) {
            return None;
        }
        let at = messages.partition_point(|m| (m.at, &m.id) <= (message.at, &message.id));
        messages.insert(at, message.clone());
        Some(day_bytes(&messages))
    })
}

/// Changes a message of `from`; someone else's message stays as it is.
fn change(room: &str, day: &str, msg: &str, from: &str, apply: impl Fn(&mut Message)) -> Result<()> {
    modify(&day_path(room, day), |bytes| {
        let mut messages = parse_day(bytes?);
        let message = messages.iter_mut().find(|m| m.id == msg && m.from == from && !m.deleted)?;
        apply(message);
        Some(day_bytes(&messages))
    })
}

pub fn edit(room: &str, day: &str, msg: &str, from: &str, text: &str) -> Result<()> {
    change(room, day, msg, from, |m| {
        m.text = text.to_owned();
        m.edited = true;
    })
}

pub fn delete(room: &str, day: &str, msg: &str, from: &str) -> Result<()> {
    change(room, day, msg, from, |m| {
        m.text.clear();
        m.deleted = true;
    })
}

pub fn carry_out(effects: Vec<Effect>) -> Result<()> {
    for effect in effects {
        match effect {
            Effect::Append { room, message } => append(&room, message)?,
            Effect::Edit { room, day, msg, from, text } => edit(&room, &day, &msg, &from, &text)?,
            Effect::Delete { room, day, msg, from } => delete(&room, &day, &msg, &from)?,
        }
    }
    Ok(())
}

/// The days of a room that have messages, newest first.
pub fn days(room: &str) -> Result<Vec<String>> {
    let entries = match appdata::list(&format!("rooms/{room}")) {
        Ok(entries) => entries,
        Err(appdata::Error::NotFound) => return Ok(Vec::new()),
        Err(e) => return Err(format!("{room}: {e:?}")),
    };
    let mut days: Vec<String> = entries
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| e.name.strip_suffix(".jsonl").map(str::to_owned))
        .collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    Ok(days)
}

/// The last messages of a room before the day `before` (from the newest when
/// empty): whole days, until at least `want` messages. The second value is
/// the `before` for the next page, `None` when there are no older days.
pub fn recent(room: &str, before: &str, want: usize) -> Result<(Vec<Message>, Option<String>)> {
    let days: Vec<String> = days(room)?.into_iter().filter(|d| before.is_empty() || d.as_str() < before).collect();
    let mut pages: Vec<Vec<Message>> = Vec::new();
    let mut count = 0;
    let mut older = None;
    for (i, day) in days.iter().enumerate() {
        let messages = read_day(room, day)?;
        count += messages.len();
        pages.push(messages);
        if count >= want {
            older = (i + 1 < days.len()).then(|| day.clone());
            break;
        }
    }
    pages.reverse();
    Ok((pages.concat(), older))
}
