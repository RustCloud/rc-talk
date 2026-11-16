//! How envelopes reach other users.
//!
//! Target (core API 0.2.19+, maintainers' answers): `jobs.add-for` puts a job
//! for each recipient; the server runs it as the recipient, with only AppData,
//! stamps `from` (with the account id) and `created-at`. Then [`send`] becomes
//! `add-for` and [`take_incoming`] moves into `run-batch`.
//!
//! Until then — **temporary, for the test bench only, never in the catalog**,
//! behind the server setting `storage_delivery` (off by default): envelopes
//! wait in the plugin's storage until the recipient opens Talk, which moves
//! them into the recipient's AppData and deletes them.
//!
//! - `inbox/<account id>/<time>-<id>.json` — to a known person;
//! - `asks/<username>/<time>-<id>.json` — contact requests, the only thing
//!   sent to a username (its id comes with the answer).
//!
//! What this lacks compared to `add-for`, as the setting's hint says: messages
//! wait in shared storage rather than with the user; `from` is set by this
//! plugin, not the server; no server-side "who may write to whom"; a deleted
//! account's folder stays until the administrator clears the storage (ids
//! are never reused, so nobody else gets it).

use serde::{Deserialize, Serialize};

use crate::inbox;
use crate::model::*;
use crate::rustcloud::app::{log, settings, storage};
use crate::store::{self, Result};
use crate::User;

/// The most envelopes waiting for one person in one folder; more are dropped
/// (storage has no quotas, and anyone could fill it).
pub const MAX_WAITING: usize = 2000;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Nothing reaches other users.
    Off,
    /// The temporary delivery through storage.
    Storage,
}

pub fn mode() -> Mode {
    let values: serde_json::Value = serde_json::from_str(&settings::values()).unwrap_or_default();
    if values["storage_delivery"].as_bool() == Some(true) {
        Mode::Storage
    } else {
        Mode::Off
    }
}

#[derive(Serialize, Deserialize)]
struct Waiting {
    from: Sender,
    envelope: Envelope,
}

pub fn sender(user: &User) -> Sender {
    Sender { id: user.id.clone(), username: user.username.clone(), display_name: user.display_name.clone() }
}

fn put(user: &User, dirs: impl Iterator<Item = String>, envelope: &Envelope) -> Result<()> {
    if mode() != Mode::Storage {
        return Err("delivery is off".into());
    }
    let waiting =
        serde_json::to_vec(&Waiting { from: sender(user), envelope: envelope.clone() }).map_err(|e| e.to_string())?;
    if waiting.len() > MAX_ENVELOPE + 512 {
        return Err("envelope too large".into());
    }
    let name = format!("{:013}-{}.json", envelope.sent_at.timestamp_millis().max(0), envelope.id);
    for dir in dirs {
        let full = storage::list(&dir).map(|files| files.len() >= MAX_WAITING).unwrap_or(false);
        if full {
            // Same answer as success: the sender learns nothing about the recipient.
            log::warn(&format!("talk: {dir} is full, an envelope from {} dropped", user.username));
            continue;
        }
        storage::write(&format!("{dir}/{name}"), &waiting).map_err(|e| format!("{e:?}"))?;
    }
    Ok(())
}

/// Sends `envelope` to each of `to` (account ids); the sender is skipped.
pub fn send(user: &User, to: &[String], envelope: &Envelope) -> Result<()> {
    let dirs = to.iter().filter(|id| **id != user.id && valid_user_id(id)).map(|id| format!("inbox/{id}"));
    put(user, dirs, envelope)
}

/// Sends a contact request to a username. An unknown username is not an
/// error: nobody will ever take it.
pub fn ask(user: &User, username: &str, envelope: &Envelope) -> Result<()> {
    if !valid_username(username) || username == user.username {
        return Err("bad username".into());
    }
    put(user, std::iter::once(format!("asks/{username}")), envelope)
}

/// Moves what waits for `user` into their AppData. Returns how many envelopes
/// were taken (applied or dropped by the rules).
pub fn take_incoming(user: &User) -> Result<usize> {
    if mode() != Mode::Storage || !valid_user_id(&user.id) || !valid_username(&user.username) {
        return Ok(0);
    }
    let mut files = Vec::new();
    for dir in [format!("inbox/{}", user.id), format!("asks/{}", user.username)] {
        match storage::list(&dir) {
            Ok(found) => files.extend(found),
            Err(storage::Error::NotFound) => {}
            Err(e) => return Err(format!("{e:?}")),
        }
    }
    if files.is_empty() {
        return Ok(0);
    }
    // By time, then id: the file name is `<time>-<id>.json`.
    files.sort_unstable_by(|a, b| a.path.rsplit('/').next().cmp(&b.path.rsplit('/').next()));
    files.truncate(500);

    let mut state = store::load(sender(user))?;
    let now = chrono::Utc::now();
    let mut taken = Vec::new();
    for file in &files {
        let bytes = storage::read(&file.path, 0, file.size.max(1)).map_err(|e| format!("{e:?}"))?;
        if let Ok(waiting) = serde_json::from_slice::<Waiting>(&bytes) {
            let effects = inbox::apply(&mut state, &waiting.from, &waiting.envelope, now);
            store::carry_out(effects)?;
        }
        taken.push(file.path.clone());
    }
    store::save(&state)?;
    // Deleted only once saved: a failure above leaves them to be taken again,
    // and taking twice is harmless (messages are kept by id).
    for path in &taken {
        let _ = storage::delete(path);
    }
    Ok(taken.len())
}
