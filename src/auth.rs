use crate::config::Config;
use aes_gcm::aead::{Aead, consts::U12};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::{RngCore, rng};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use url::form_urlencoded;

const SCOPES: &str = "openid email profile https://www.googleapis.com/auth/youtube.readonly";

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub email: String,
    pub avatar: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub expiry: u64,
    pub profile: Option<Profile>,
}

#[derive(Serialize, Deserialize)]
struct Pkce {
    verifier: String,
    state: String,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn random_token(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn cipher(config: &Config) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(&config.session_secret).expect("32-byte session key")
}

pub fn seal<T: Serialize>(value: &T, config: &Config) -> Result<String, String> {
    let mut iv = [0u8; 12];
    rng().fill_bytes(&mut iv);
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let nonce = Nonce::<U12>::from_slice(&iv);
    let ciphertext = cipher(config)
        .encrypt(nonce, payload.as_slice())
        .map_err(|_| "encrypt failed".to_string())?;
    let mut raw = Vec::with_capacity(12 + ciphertext.len());
    raw.extend_from_slice(&iv);
    raw.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

pub fn unseal<T: DeserializeOwned>(token: &str, config: &Config) -> Option<T> {
    let raw = URL_SAFE_NO_PAD.decode(token).ok()?;
    if raw.len() < 29 {
        return None;
    }
    let nonce = Nonce::<U12>::from_slice(&raw[0..12]);
    cipher(config)
        .decrypt(nonce, &raw[12..])
        .ok()
        .and_then(|plain| serde_json::from_slice(&plain).ok())
}

fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

pub fn parse_cookies(header: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(header) = header else { return out };
    for part in header.split(';') {
        if let Some((name, value)) = part.trim().split_once('=') {
            out.insert(name.to_string(), value.to_string());
        }
    }
    out
}

pub fn cookie(name: &str, value: &str, max_age: i64, config: &Config) -> String {
    let mut parts = vec![
        format!("{name}={value}"),
        "Path=/".to_string(),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
        format!("Max-Age={max_age}"),
    ];
    if config.secure_cookies {
        parts.push("Secure".to_string());
    }
    parts.join("; ")
}

pub fn auth_url(config: &Config) -> Result<(String, String), String> {
    if config.google_client_id.is_empty() || config.google_client_secret.is_empty() {
        return Err(
            "Set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET to enable Google sign-in.".into(),
        );
    }
    let verifier = random_token(32);
    let state = random_token(16);
    let pkce = Pkce {
        verifier,
        state: state.clone(),
    };
    let cookie_value = seal(&pkce, config)?;

    let mut url = url::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut()
        .append_pair("client_id", &config.google_client_id)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPES)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("code_challenge", &pkce_challenge(&pkce.verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);

    Ok((
        url.to_string(),
        cookie("st_pkce", &cookie_value, 600, config),
    ))
}

pub fn callback(
    code: &str,
    state: &str,
    cookies: &HashMap<String, String>,
    config: &Config,
) -> Result<(Session, String, String), String> {
    if config.google_client_id.is_empty() || config.google_client_secret.is_empty() {
        return Err("Google OAuth is not configured.".into());
    }
    let pkce: Pkce = cookies
        .get("st_pkce")
        .and_then(|value| unseal(value, config))
        .ok_or_else(|| "OAuth callback was missing a valid PKCE cookie.".to_string())?;
    if state != pkce.state {
        return Err("OAuth callback state did not match.".into());
    }

    let tokens = post_form(
        "https://oauth2.googleapis.com/token",
        &[
            ("code", code),
            ("client_id", &config.google_client_id),
            ("client_secret", &config.google_client_secret),
            ("redirect_uri", &config.redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", &pkce.verifier),
        ],
    )?;
    let access_token = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if access_token.is_empty() {
        return Err("Google did not return an access token.".into());
    }
    let profile = profile_from_token(
        &access_token,
        tokens.get("id_token").and_then(Value::as_str),
    );
    let session = Session {
        access_token,
        refresh_token,
        expiry: now()
            + tokens
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(3600),
        profile,
    };
    let session_cookie = cookie(
        "st_session",
        &seal(&session, config)?,
        60 * 60 * 24 * 30,
        config,
    );
    let clear_pkce = cookie("st_pkce", "", 0, config);
    Ok((session, session_cookie, clear_pkce))
}

pub fn refresh(session: &Session, config: &Config) -> Result<Session, String> {
    if session.refresh_token.is_empty() {
        return Err("No refresh token.".into());
    }
    let tokens = post_form(
        "https://oauth2.googleapis.com/token",
        &[
            ("client_id", &config.google_client_id),
            ("client_secret", &config.google_client_secret),
            ("grant_type", "refresh_token"),
            ("refresh_token", &session.refresh_token),
        ],
    )?;
    Ok(Session {
        access_token: tokens
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or(&session.access_token)
            .to_string(),
        refresh_token: tokens
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or(&session.refresh_token)
            .to_string(),
        expiry: now()
            + tokens
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(3600),
        profile: session.profile.clone(),
    })
}

pub fn logout_cookies(config: &Config) -> Vec<String> {
    vec![
        cookie("st_session", "", 0, config),
        cookie("st_pkce", "", 0, config),
    ]
}

pub fn load_session(
    cookies: &HashMap<String, String>,
    config: &Config,
) -> (Option<Session>, Option<String>) {
    let Some(token) = cookies.get("st_session") else {
        return (None, None);
    };
    let Some(session): Option<Session> = unseal(token, config) else {
        return (None, None);
    };
    if session.expiry > now() + 30 {
        return (Some(session), None);
    }
    match refresh(&session, config) {
        Ok(refreshed) => {
            let set_cookie = seal(&refreshed, config)
                .ok()
                .map(|value| cookie("st_session", &value, 60 * 60 * 24 * 30, config));
            (Some(refreshed), set_cookie)
        }
        Err(_) => (None, None),
    }
}

fn profile_from_token(access_token: &str, id_token: Option<&str>) -> Option<Profile> {
    if let Some(id_token) = id_token {
        let url = format!("https://oauth2.googleapis.com/tokeninfo?id_token={id_token}");
        if let Ok(value) = get_json(&url, &[]) {
            let id = value
                .get("sub")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !id.is_empty() {
                return Some(Profile {
                    id,
                    name: value
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| value.get("email").and_then(Value::as_str))
                        .unwrap_or("Signed in")
                        .to_string(),
                    email: value
                        .get("email")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    avatar: value
                        .get("picture")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }

    let value = get_json(
        "https://www.googleapis.com/oauth2/v3/userinfo",
        &[("Authorization", &format!("Bearer {access_token}"))],
    )
    .ok()?;
    Some(Profile {
        id: value
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Signed in")
            .to_string(),
        email: value
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        avatar: value
            .get("picture")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

pub fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<Value, String> {
    let body = form(fields);
    let mut response = crate::api::agent()
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .send(body)
        .map_err(|error| error.to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    if let Some(message) = value.get("error_description").and_then(Value::as_str) {
        return Err(message.to_string());
    }
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Err(message.to_string());
    }
    Ok(value)
}

pub fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut request = crate::api::agent()
        .get(url)
        .header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().map_err(|error| error.to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

pub fn form(fields: &[(&str, &str)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}
