//! How envelopes reach other users: `jobs.add-for` adds a job `envelope` for
//! each recipient. The server runs it as the recipient ([`run_batch`]), with
//! AppData only, and says who added it and when (`jobs.origins`): a sender
//! can't pretend to be someone else.
//!
//! Recipients the sender may not reach — unknown, blocked, kept apart by the
//! administrator — get nothing, and the sender gets the same answer: it
//! doesn't tell who exists. So nothing is answered automatically either.

use chrono::{DateTime, Utc};

use crate::inbox;
use crate::model::*;
use crate::rustcloud::app::jobs::{self, Job, NewJob, Outcome, SendError};
use crate::rustcloud::app::{log, notify};
use crate::store;
use crate::User;

/// The job kind of `permissions.deliver`.
pub const KIND: &str = "envelope";

/// Recipients one `add-for` takes.
const PER_CALL: usize = 50;

pub enum Error {
    /// The sender sends too much too fast.
    TooMany,
    Failed(String),
}

pub fn sender(user: &User) -> Sender {
    Sender { id: user.id.clone(), username: user.username.clone(), display_name: user.display_name.clone() }
}

/// Sends `envelope` to each of `to` (account ids or, for a contact request,
/// a username); the sender is skipped.
pub fn send(user: &User, to: &[String], envelope: &Envelope) -> Result<(), Error> {
    let mut recipients: Vec<String> = Vec::new();
    for id in to {
        if *id != user.id && !recipients.contains(id) {
            recipients.push(id.clone());
        }
    }
    if recipients.is_empty() {
        return Ok(());
    }
    let data = serde_json::to_string(envelope).map_err(|e| Error::Failed(e.to_string()))?;
    if data.len() > MAX_ENVELOPE {
        return Err(Error::Failed("envelope too large".into()));
    }
    // The envelope's id as the key: a later envelope never replaces a waiting one.
    let job = NewJob { kind: KIND.into(), key: envelope.id.clone(), data };
    for chunk in recipients.chunks(PER_CALL) {
        match jobs::add_for(chunk, &job) {
            Ok(()) => {}
            Err(SendError::TooMany) => return Err(Error::TooMany),
            Err(e) => return Err(Error::Failed(format!("add-for: {e:?}"))),
        }
    }
    Ok(())
}

/// Sets the number by Talk in the user's side panel.
pub fn badge(count: u32) {
    if let Err(e) = notify::badge(count) {
        log::warn(&format!("talk: badge: {e:?}"));
    }
}

/// Takes a batch of envelopes others sent to `user`.
pub fn run_batch(user: &User, batch: &[Job]) -> Vec<Outcome> {
    let origins = jobs::origins();
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(batch.len());
    let mut incoming: Vec<(DateTime<Utc>, usize, Sender, Envelope)> = Vec::new();
    for (i, job) in batch.iter().enumerate() {
        let Some(origin) = origins.get(i) else {
            outcomes.push(Outcome::Failed("no origin".into()));
            continue;
        };
        if job.kind != KIND {
            outcomes.push(Outcome::Failed(format!("unknown kind {}", job.kind)));
            continue;
        }
        // Done either way: one that can't be read never will be.
        outcomes.push(Outcome::Done);
        let Ok(envelope) = serde_json::from_str::<Envelope>(&job.data) else { continue };
        let added = DateTime::parse_from_rfc3339(&origin.created_at).map(|t| t.to_utc()).unwrap_or_else(|_| Utc::now());
        let from = Sender {
            id: origin.from.id.clone(),
            username: origin.from.username.clone(),
            display_name: origin.from.display_name.clone(),
        };
        incoming.push((added, i, from, envelope));
    }
    // A retried job keeps its time: put them back in the order they were sent.
    incoming.sort_by_key(|(added, i, _, _)| (*added, *i));

    let now = Utc::now();
    let taken = store::update(&sender(user), |state| {
        let effects: Vec<inbox::Effect> =
            incoming.iter().flat_map(|(_, _, from, envelope)| inbox::apply(state, from, envelope, now)).collect();
        (effects, state.waiting())
    })
    .and_then(|(effects, waiting)| store::carry_out(effects).map(|()| waiting));
    match taken {
        Ok(waiting) => {
            badge(waiting);
            outcomes
        }
        Err(e) => {
            log::warn(&format!("talk: delivery to {}: {e}", user.username));
            batch.iter().map(|_| Outcome::Retry(e.clone())).collect()
        }
    }
}
