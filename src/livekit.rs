//! LiveKit: every call, two people too, is a LiveKit room. Talk checks that the user is a member of the chat and signs a JWT
//! (HS256) with the API key and secret from the server settings; the secret
//! never reaches the browser.
//!
//! - Identity — the account id (a username can pass to someone else), name —
//!   "Display (username)", never the display name alone.
//! - Tokens live [`TOKEN_SECONDS`]: LiveKit checks a token only on joining and
//!   its SDK renews it while connected, so someone taken out of a chat can't
//!   join for hours afterwards. Someone taken out during a call is removed
//!   with RoomService `RemoveParticipant` ([`remove_request`]).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

/// How long a join token is valid.
pub const TOKEN_SECONDS: i64 = 5 * 60;

pub struct Config {
    /// `wss://…`, what the browser connects to.
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
    /// `http(s)://…`, how the server reaches LiveKit's API (may be an
    /// internal address); `None` — calls work, removing from a call doesn't.
    pub api_url: Option<String>,
}

impl Config {
    /// From the server settings JSON; `None` until the address, key and
    /// secret are filled in.
    pub fn from_settings(values: &Value) -> Option<Config> {
        let get = |key: &str| values[key].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        Some(Config {
            url: get("livekit_url")?,
            api_key: get("livekit_api_key")?,
            api_secret: get("livekit_api_secret")?,
            api_url: get("livekit_api_url").map(|url| url.trim_end_matches('/').to_owned()),
        })
    }
}

/// The LiveKit room of a chat.
pub fn room_name(room: &str) -> String {
    format!("talk-{room}")
}

fn sign(config: &Config, claims: &Value) -> String {
    let header = json!({ "alg": "HS256", "typ": "JWT" });
    let encode = |value: &Value| URL_SAFE_NO_PAD.encode(value.to_string());
    let unsigned = format!("{}.{}", encode(&header), encode(claims));
    let mut mac = Hmac::<Sha256>::new_from_slice(config.api_secret.as_bytes()).expect("HMAC takes any key");
    mac.update(unsigned.as_bytes());
    format!("{unsigned}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// A token to join the call of `room` as `identity` (account id) shown as
/// `name`; `now` in Unix seconds.
pub fn join_token(config: &Config, room: &str, identity: &str, name: &str, now: i64) -> String {
    sign(
        config,
        &json!({
            "iss": config.api_key,
            "sub": identity,
            "name": name,
            "nbf": now - 10,
            "exp": now + TOKEN_SECONDS,
            "video": {
                "room": room_name(room),
                "roomJoin": true,
                "canPublish": true,
                "canSubscribe": true,
                "canPublishData": true,
            },
        }),
    )
}

/// An HTTP request to send through the `http` import: URL, headers, body.
pub type ApiRequest = (String, Vec<(String, String)>, Vec<u8>);

/// The request that removes `identity` from the call
/// of `room`; `None` without `api_url`. Sent by the caller through `http`.
pub fn remove_request(config: &Config, room: &str, identity: &str, now: i64) -> Option<ApiRequest> {
    let api = config.api_url.as_ref()?;
    let token = sign(
        config,
        &json!({
            "iss": config.api_key,
            "nbf": now - 10,
            "exp": now + 60,
            "video": { "room": room_name(room), "roomAdmin": true },
        }),
    );
    let body = json!({ "room": room_name(room), "identity": identity }).to_string().into_bytes();
    Some((
        format!("{api}/twirp/livekit.RoomService/RemoveParticipant"),
        vec![
            ("authorization".to_owned(), format!("Bearer {token}")),
            ("content-type".to_owned(), "application/json".to_owned()),
        ],
        body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            url: "wss://lk".into(),
            api_key: "key".into(),
            api_secret: "secret".into(),
            api_url: Some("http://lk:7880".into()),
        }
    }

    fn claims(token: &str) -> Value {
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(parts[2]).unwrap()).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap()
    }

    #[test]
    fn join_token_is_short_and_for_one_room() {
        let claims = claims(&join_token(&config(), "d0a1", "id-alice", "Alice (alice)", 1_000));
        assert_eq!(claims["sub"], "id-alice");
        assert_eq!(claims["exp"], 1_000 + TOKEN_SECONDS);
        assert_eq!(claims["video"]["room"], "talk-d0a1");
        assert!(claims["video"]["roomAdmin"].is_null());
    }

    #[test]
    fn remove_request_is_an_admin_call_for_that_room() {
        let (url, headers, body) = remove_request(&config(), "g1", "id-carol", 1_000).unwrap();
        assert_eq!(url, "http://lk:7880/twirp/livekit.RoomService/RemoveParticipant");
        let token = headers[0].1.strip_prefix("Bearer ").unwrap();
        assert_eq!(claims(token)["video"]["roomAdmin"], true);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["identity"], "id-carol");

        let without_api = Config { api_url: None, ..config() };
        assert!(remove_request(&without_api, "g1", "id-carol", 1_000).is_none());
    }

    #[test]
    fn config_needs_address_key_and_secret() {
        let partial = json!({ "livekit_url": "wss://lk", "livekit_api_key": "k" });
        assert!(Config::from_settings(&partial).is_none());
        let full = json!({ "livekit_url": "wss://lk", "livekit_api_key": "k", "livekit_api_secret": "s" });
        assert!(Config::from_settings(&full).unwrap().api_url.is_none());
    }
}
