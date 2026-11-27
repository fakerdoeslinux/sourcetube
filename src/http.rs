use crate::api;
use crate::auth::{self, Session};
use crate::config::Config;
use crate::mapping;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tiny_http::{Header, Request, Response};
use url::Url;

const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' https://www.youtube-nocookie.com; frame-src https://www.youtube-nocookie.com; connect-src 'self'; font-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("Referrer-Policy", "strict-origin-when-cross-origin"),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    (
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=(), interest-cohort=()",
    ),
];

pub fn handle(request: Request, config: &Config) {
    let host = request_header(&request, "Host").unwrap_or_else(|| "localhost".to_string());
    let url = Url::parse(&format!("http://{host}{}", request.url()))
        .unwrap_or_else(|_| Url::parse("http://localhost/").unwrap());
    let path = url.path().to_string();

    if path.starts_with("/api/") {
        if let Some(source) = binary_source(&path, &url, config) {
            match source {
                Ok(Some((location, content_type, cache))) => {
                    match api::fetch_binary(&location) {
                        Ok((body, upstream_type)) => {
                            let content_type = if content_type.is_empty() {
                                upstream_type
                            } else {
                                content_type
                            };
                            reply(
                                request,
                                200,
                                body,
                                &content_type,
                                vec![("Cache-Control".to_string(), cache)],
                            );
                        }
                        Err(error) => {
                            json_reply(request, 502, json!({ "error": error }), Vec::new())
                        }
                    }
                    return;
                }
                Ok(None) => {}
                Err((status, message)) => {
                    json_reply(request, status, json!({ "error": message }), Vec::new());
                    return;
                }
            }
        }

        let cookies = auth::parse_cookies(request_header(&request, "Cookie").as_deref());
        let (session, refreshed_cookie) = auth::load_session(&cookies, config);
        let mut extra = Vec::new();
        if let Some(cookie) = refreshed_cookie {
            extra.push(("Set-Cookie".to_string(), cookie));
        }

        match api_route(&url, &path, session, config, &cookies, &mut extra) {
            Ok((status, value)) => json_reply(request, status, value, extra),
            Err((302, location)) => {
                extra.push(("Location".to_string(), location));
                reply(request, 302, Vec::new(), "text/plain; charset=utf-8", extra);
            }
            Err((status, message)) => {
                json_reply(request, status, json!({ "error": message }), extra)
            }
        }
        return;
    }

    static_reply(request, &path, config);
}

fn api_route(
    url: &Url,
    path: &str,
    session: Option<Session>,
    config: &Config,
    cookies: &std::collections::HashMap<String, String>,
    extra: &mut Vec<(String, String)>,
) -> Result<(u16, Value), (u16, String)> {
    if path == "/api/config" {
        let profile = session.as_ref().and_then(|value| value.profile.clone());
        return Ok((
            200,
            json!({
                "signedIn": session.is_some(),
                "profile": profile,
                "hasOAuth": !config.google_client_id.is_empty() && !config.google_client_secret.is_empty(),
                "hasYouTubeKey": !config.youtube_api_key.is_empty(),
                "metadataApi": "FreeTube-compatible Invidious",
                "privacy": {
                    "analytics": false,
                    "trackers": false,
                    "clientIpToGoogleForMetadata": false
                }
            }),
        ));
    }

    if path == "/api/me" {
        return Ok((
            200,
            json!({ "profile": session.as_ref().and_then(|value| value.profile.clone()) }),
        ));
    }

    if path == "/api/auth/url" {
        let (location, cookie) = auth::auth_url(config).map_err(|error| (503, error))?;
        extra.push(("Set-Cookie".to_string(), cookie));
        return Ok((200, json!({ "url": location })));
    }

    if path == "/api/auth/callback" {
        let code = param(url, "code");
        let state = param(url, "state");
        let (_new_session, session_cookie, clear_cookie) =
            auth::callback(&code, &state, cookies, config).map_err(|error| (400, error))?;
        extra.push(("Set-Cookie".to_string(), session_cookie));
        extra.push(("Set-Cookie".to_string(), clear_cookie));
        return Err((302, format!("{}/#signedin=1", config.app_origin)));
    }

    if path == "/api/auth/logout" {
        for cookie in auth::logout_cookies(config) {
            extra.push(("Set-Cookie".to_string(), cookie));
        }
        return Err((302, format!("{}/#signedout=1", config.app_origin)));
    }

    if path == "/api/popular" {
        let response =
            api::invidious("/api/v1/popular", &[], config).map_err(|error| (502, error))?;
        let videos = as_array(&response)
            .iter()
            .filter(|item| item.get("videoId").is_some())
            .map(mapping::map_video)
            .collect::<Vec<_>>();
        return Ok((200, json!({ "videos": videos })));
    }

    if path == "/api/search" {
        let query = param(url, "q");
        if query.trim().is_empty() {
            return Ok((200, json!({ "videos": [] })));
        }
        let response = api::invidious(
            "/api/v1/search",
            &[
                ("q", &query),
                ("type", "video"),
                ("sort_by", "relevance"),
                ("page", "1"),
            ],
            config,
        )
        .map_err(|error| (502, error))?;
        let videos = as_array(&response)
            .iter()
            .filter(|item| item.get("videoId").is_some())
            .map(mapping::map_video)
            .collect::<Vec<_>>();
        return Ok((200, json!({ "videos": videos })));
    }

    if path == "/api/video" {
        let id = param(url, "id");
        if !mapping::valid_video_id(&id) {
            return Err((400, "Invalid video ID.".into()));
        }
        let response = api::invidious(&format!("/api/v1/videos/{id}"), &[], config)
            .map_err(|error| (502, error))?;
        return Ok((200, json!({ "video": mapping::map_video(&response) })));
    }

    if let Some(id) = path.strip_prefix("/api/related/") {
        if !mapping::valid_video_id(id) {
            return Err((400, "Invalid video ID.".into()));
        }
        let response = api::invidious(&format!("/api/v1/videos/{id}"), &[], config)
            .map_err(|error| (502, error))?;
        let empty: Vec<Value> = Vec::new();
        let videos = response
            .get("recommendedVideos")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
            .iter()
            .filter(|item| item.get("videoId").is_some())
            .map(mapping::map_video)
            .collect::<Vec<_>>();
        return Ok((200, json!({ "videos": videos })));
    }

    if path == "/api/stream" {
        let id = param(url, "id");
        if !mapping::valid_video_id(&id) {
            return Err((400, "Invalid video ID.".into()));
        }
        let response = api::invidious(&format!("/api/v1/videos/{id}"), &[], config)
            .map_err(|error| (502, error))?;
        let instance = config
            .invidious_instances
            .first()
            .map(String::as_str)
            .unwrap_or("");
        let stream_url = mapping::best_stream(&response, instance).ok_or_else(|| {
            (
                404,
                "No progressive stream is available for this video.".to_string(),
            )
        })?;
        return Ok((
            200,
            json!({
                "url": stream_url,
                "id": id,
                "title": response.get("title").and_then(Value::as_str).unwrap_or("Untitled video"),
                "source": "FreeTube-compatible Invidious"
            }),
        ));
    }

    if path == "/api/launch" {
        let id = param(url, "id");
        if !mapping::valid_video_id(&id) {
            return Err((400, "Invalid video ID.".into()));
        }
        launch_player(&id, config).map_err(|error| (500, error))?;
        return Ok((200, json!({ "launched": true, "id": id })));
    }

    if path == "/api/subscriptions" {
        let session = session.ok_or_else(|| (401, "Sign in to see subscriptions.".to_string()))?;
        let videos = api::subscription_videos(&session, config).map_err(|error| (502, error))?;
        return Ok((200, json!({ "videos": videos })));
    }

    Err((404, "Unknown API endpoint.".into()))
}

fn launch_player(id: &str, config: &Config) -> Result<(), String> {
    let mut candidates = Vec::new();
    if let Ok(value) = std::env::var("SOURCETUBE_PLAYER") {
        if !value.is_empty() {
            candidates.push(PathBuf::from(value));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("sourcetube-player"));
            candidates.push(parent.join("../share/sourcetube/sourcetube-player"));
        }
    }
    candidates.push(config.public_dir.join("../bin/sourcetube-player"));
    candidates.push(PathBuf::from("sourcetube-player"));

    let player = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| "Native player binary was not found.".to_string())?;
    Command::new(player)
        .arg(id)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start native player: {error}"))
}

fn binary_source(
    path: &str,
    url: &Url,
    _config: &Config,
) -> Option<Result<Option<(String, String, String)>, (u16, String)>> {
    if let Some(rest) = path.strip_prefix("/api/thumb/") {
        let mut parts = rest.split('/');
        let id = parts.next().unwrap_or("");
        let name = parts.next().unwrap_or("");
        let name = name.strip_suffix(".jpg").unwrap_or(name);
        if !mapping::valid_video_id(id) {
            return Some(Err((400, "Invalid video ID.".into())));
        }
        let allowed = [
            "default",
            "mqdefault",
            "hqdefault",
            "sddefault",
            "maxresdefault",
        ];
        if !allowed.contains(&name) {
            return Some(Err((400, "Invalid thumbnail size.".into())));
        }
        return Some(Ok(Some((
            format!("https://i.ytimg.com/vi/{id}/{name}.jpg"),
            "image/jpeg".to_string(),
            "public, max-age=86400, immutable".to_string(),
        ))));
    }

    if path == "/api/avatar" {
        let raw = param(url, "url");
        let Ok(parsed) = Url::parse(&raw) else {
            return Some(Err((400, "Invalid avatar URL.".into())));
        };
        let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
        if parsed.scheme() != "https"
            || !(host == "googleusercontent.com" || host.ends_with(".googleusercontent.com"))
        {
            return Some(Err((400, "Avatar host is not allowed.".into())));
        }
        return Some(Ok(Some((
            raw,
            String::new(),
            "public, max-age=86400".to_string(),
        ))));
    }

    None
}

fn as_array(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn param(url: &Url, name: &str) -> String {
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

fn request_header(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|item| item.field.as_str().to_string().eq_ignore_ascii_case(name))
        .map(|item| item.value.to_string())
}

fn json_reply(request: Request, status: u16, value: Value, extra: Vec<(String, String)>) {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    let redirect = status == 302;
    let content_type = if redirect {
        "text/plain; charset=utf-8"
    } else {
        "application/json; charset=utf-8"
    };
    reply(request, status, body, content_type, extra);
}

fn static_reply(request: Request, path: &str, config: &Config) {
    let relative = if path == "/" {
        "index.html"
    } else {
        path.trim_start_matches('/')
    };
    let file = config.public_dir.join(relative);
    let root = fs::canonicalize(&config.public_dir).unwrap_or_else(|_| config.public_dir.clone());
    let canonical = fs::canonicalize(&file);
    let Ok(canonical) = canonical else {
        reply(
            request,
            404,
            b"Not found.".to_vec(),
            "text/plain; charset=utf-8",
            Vec::new(),
        );
        return;
    };
    if !canonical.starts_with(root) {
        reply(
            request,
            403,
            b"Forbidden.".to_vec(),
            "text/plain; charset=utf-8",
            Vec::new(),
        );
        return;
    }
    match fs::read(&canonical) {
        Ok(body) => {
            let content_type = mime(&canonical);
            reply(
                request,
                200,
                body,
                content_type,
                vec![(
                    "Cache-Control".to_string(),
                    "public, max-age=300".to_string(),
                )],
            );
        }
        Err(_) => reply(
            request,
            404,
            b"Not found.".to_vec(),
            "text/plain; charset=utf-8",
            Vec::new(),
        ),
    }
}

fn mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn reply(
    request: Request,
    status: u16,
    body: Vec<u8>,
    content_type: &str,
    extra: Vec<(String, String)>,
) {
    let mut response = Response::from_data(body).with_status_code(status);
    response = response.with_header(response_header("Content-Type", content_type));
    if !extra
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("Cache-Control"))
    {
        response = response.with_header(response_header("Cache-Control", "no-store"));
    }
    for (name, value) in SECURITY_HEADERS {
        response = response.with_header(response_header(name, value));
    }
    for (name, value) in extra {
        if name.eq_ignore_ascii_case("Location") && status != 302 {
            continue;
        }
        response = response.with_header(response_header(&name, &value));
    }
    if let Err(error) = request.respond(response) {
        eprintln!("SourceTube response error: {error}");
    }
}

fn response_header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}
