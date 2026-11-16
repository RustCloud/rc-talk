//! Talk: chats between the cloud's users, kept in each user's AppData, and
//! calls through LiveKit. Architecture: docs/design.md.
//!
//! | path        | what                                              |
//! |-------------|---------------------------------------------------|
//! | `/`         | the page (`/app.js`, `/app.css`)                  |
//! | `/api/…`    | its JSON API ([`api`])                            |
//! | `/status`   | the administrator's Status tab                    |

// The bindings of API 0.2.19 have a function of 8 arguments.
#![allow(clippy::too_many_arguments)]

wit_bindgen::generate!({ path: "wit", world: "app-with-jobs" });

pub mod api;
pub mod delivery;
pub mod inbox;
pub mod livekit;
pub mod model;
pub mod store;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use serde_json::{json, Value};

struct App;

impl Guest for App {
    fn handle(user: Option<User>, request: Request) -> Response {
        let path = request.path.as_str();
        match (request.method.as_str(), path) {
            ("GET", "/") => asset("text/html", include_str!("../ui/index.html")),
            ("GET", "/app.js") => asset("text/javascript", include_str!("../ui/app.js")),
            ("GET", "/app.css") => asset("text/css", include_str!("../ui/app.css")),
            ("GET", "/status") => json(200, &status()),
            (method, _) if path.starts_with("/api/") => match &user {
                Some(user) => api::handle(user, method, &path["/api".len()..], &request.query, &request.body),
                None => json(401, &json!({ "error": "NOT_LOGGED_IN" })),
            },
            _ => json(404, &json!({ "error": "NOT_FOUND" })),
        }
    }

    /// Envelopes others sent to `user` (`jobs.add-for`).
    fn run_batch(user: User, jobs: Vec<Job>) -> Vec<Outcome> {
        delivery::run_batch(&user, &jobs)
    }
}

export!(App);

/// The Status tab: what works and what the administrator should do.
fn status() -> Value {
    let values: Value = serde_json::from_str(&rustcloud::app::settings::values()).unwrap_or_default();
    let mut lines = vec![json!({
        "level": "ok",
        "text": {
            "en": "Messages: delivered by the cloud. Who may write to whom is set in the plugin's system settings.",
            "ru": "Сообщения: доставляет облако. Кто кому может писать — в системных настройках плагина."
        }
    })];
    if livekit::Config::from_settings(&values).is_some() {
        lines.push(
            json!({ "level": "ok", "text": { "en": "Calls: LiveKit is set up", "ru": "Звонки: LiveKit настроен" } }),
        );
    } else {
        lines.push(json!({
            "level": "info",
            "text": {
                "en": "Calls are off: fill in the LiveKit address, key and secret.",
                "ru": "Звонки выключены: заполните адрес, ключ и секрет LiveKit."
            }
        }));
    }
    Value::Array(lines)
}

/// 128 random bits as hex. `RandomState` is seeded from the system's random
/// source (WASI `random`).
pub fn random_id() -> String {
    let part = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", part(), part())
}

/// A parameter of a query string, decoded.
pub fn query_param(query: &str, name: &str) -> String {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode(value))
        .unwrap_or_default()
}

/// Decodes `%XX`, and `+` as a space.
fn decode(value: &str) -> String {
    let hex = |byte: u8| (byte as char).to_digit(16);
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high * 16 + low) as u8);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn json(status: u16, value: &Value) -> Response {
    Response {
        status,
        headers: vec![
            ("content-type".to_owned(), "application/json".to_owned()),
            ("cache-control".to_owned(), "no-store".to_owned()),
        ],
        body: serde_json::to_vec(value).unwrap_or_default(),
    }
}

fn asset(content_type: &str, body: &str) -> Response {
    Response {
        status: 200,
        headers: vec![("content-type".to_owned(), format!("{content_type}; charset=utf-8"))],
        body: body.as_bytes().to_vec(),
    }
}
