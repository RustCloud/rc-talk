//! The user's AppData: contacts, rooms and day files of messages
//! (layout in [`crate::model`]).
//!
//! TODO(appdata.write-if-match): a request and a delivery job of one user may
//! write the same file at once. When the core has `appdata.write-if-match`,
//! read with the etag and retry on `conflict`.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::inbox::{Effect, State};
use crate::model::*;
use crate::rustcloud::app::appdata;

pub type Result<T> = std::result::Result<T, String>;

fn read_json<T: DeserializeOwned + Default>(path: &str) -> Result<T> {
    match appdata::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{path}: {e}")),
        Err(appdata::Error::NotFound) => Ok(T::default()),
        Err(e) => Err(format!("{path}: {e:?}")),
    }
}

fn write_json<T: Serialize>(path: &str, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    appdata::write(path, &bytes).map(|_| ()).map_err(|e| format!("{path}: {e:?}"))
}

pub fn load(me: Sender) -> Result<State> {
    Ok(State { me, book: read_json("contacts.json")?, rooms: read_json("rooms.json")? })
}

pub fn save(state: &State) -> Result<()> {
    write_json("contacts.json", &state.book)?;
    write_json("rooms.json", &state.rooms)
}

fn day_path(room: &str, day: &str) -> String {
    format!("rooms/{room}/{day}.jsonl")
}

pub fn read_day(room: &str, day: &str) -> Result<Vec<Message>> {
    match appdata::read(&day_path(room, day)) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()),
        Err(appdata::Error::NotFound) => Ok(Vec::new()),
        Err(e) => Err(format!("{room}/{day}: {e:?}")),
    }
}

fn write_day(room: &str, day: &str, messages: &[Message]) -> Result<()> {
    let mut out = String::new();
    for message in messages {
        out.push_str(&serde_json::to_string(message).map_err(|e| e.to_string())?);
        out.push('\n');
    }
    appdata::write(&day_path(room, day), out.as_bytes()).map(|_| ()).map_err(|e| format!("{room}/{day}: {e:?}"))
}

/// Adds a message to its day, ordered by time and then id (the order the
/// maintainers advise for jobs: retries may run after later ones); a message
/// already there (the same id delivered twice) is left as it is.
pub fn append(room: &str, message: Message) -> Result<()> {
    let day = day_of(&message.at);
    let mut messages = read_day(room, &day)?;
    if messages.iter().any(|m| m.id == message.id) {
        return Ok(());
    }
    let at = messages.partition_point(|m| (m.at, &m.id) <= (message.at, &message.id));
    messages.insert(at, message);
    write_day(room, &day, &messages)
}

/// Changes a message of `from`; someone else's message stays as it is.
fn change(room: &str, day: &str, msg: &str, from: &str, apply: impl FnOnce(&mut Message)) -> Result<()> {
    let mut messages = read_day(room, day)?;
    match messages.iter_mut().find(|m| m.id == msg && m.from == from && !m.deleted) {
        Some(message) => {
            apply(message);
            write_day(room, day, &messages)
        }
        None => Ok(()),
    }
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
