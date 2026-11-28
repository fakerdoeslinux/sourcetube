use crate::api;
use crate::auth::{self, Session};
use crate::config::Config;
use crate::dash;
use crate::guard::Purpose;
use crate::mapping;
use crate::sponsorblock;
use crate::transfer;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tiny_http::{Header, Request, Response};
use url::Url;

const SEGMENT_CAP: usize = 32 * 1024 * 1024;
const THUMBNAIL_CAP: usize = 4 * 1024 * 1024;
const AVATAR_CAP: usize = 4 * 1024 * 1024;
const MANIFEST_CAP: usize = 8 * 1024 * 1024;
const TAKEOUT_CAP: usize = 32 * 1024 * 1024;

const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self' blob:; font-src 'self'; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
    ("X-Frame-Options", "DENY"),
    ("Cross-Origin-Opener-Policy", "same-origin"),
    (
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=(), payment=(), interest-cohort=()",
    ),
];

pub fn handle(mut request: Request, config: &Config) {
    let host = request_header(&request, "Host").unwrap_or_else(|| "localhost".to_string());
    let url = Url::parse(&format!("http://{host}{}", request.url()))
        .unwrap_or_else(|_| Url::parse("http://localhost/").unwrap());
    let path = url.path().to_string();

    if path.starts_with("/api/") {
        // Same-origin DASH pipeline: no YouTube iframe, no third-party media.
        if path == "/api/dash/segment" {
            dash_segment(request, &url, config);
            return;
        }
        if path == "/api/dash/manifest" {
            dash_manifest(request, &url, config);
            return;
        }
        if path == "/api/image" {
            image_proxy(request, &url, config);
            return;
        }

        if let Some(source) = binary_source(&path, &url, config) {
            match source {
                Ok(Some((location, purpose, content_type, cache))) => {
                    match api::fetch_binary(
                        &location,
                        purpose,
                        config,
                        THUMBNAIL_CAP.max(AVATAR_CAP),
                    ) {
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

        let body = if path == "/api/transfer/takeout" {
            read_body(&mut request, TAKEOUT_CAP)
        } else {
            Vec::new()
        };
        let adblock = request_header(&request, "X-SourceTube-Adblock")
            .map(|value| value != "off" && value != "0" && value != "false")
            .unwrap_or(true);
        let cookies = auth::parse_cookies(request_header(&request, "Cookie").as_deref());
        let (session, refreshed_cookie) = auth::load_session(&cookies, config);
        let mut extra = Vec::new();
        if let Some(cookie) = refreshed_cookie {
            extra.push(("Set-Cookie".to_string(), cookie));
        }

        match api_route(
            &url, &path, session, config, &cookies, &mut extra, adblock, &body,
        ) {
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
    adblock: bool,
    body: &[u8],
) -> Result<(u16, Value), (u16, String)> {
    if path == "/api/config" {
        let profile = session.as_ref().and_then(|value| value.profile.clone());
        return Ok((
            200,
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "signedIn": session.is_some(),
                "profile": profile,
                "hasOAuth": !config.google_client_id.is_empty() && !config.google_client_secret.is_empty(),
                "hasYouTubeKey": !config.youtube_api_key.is_empty(),
                "metadataApi": "FreeTube-compatible Invidious",
                "adblock": adblock,
                "sponsorblock": true,
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
        return Err((302, format!("{}/#/signin", config.app_origin)));
    }

    if path == "/api/auth/logout" {
        for cookie in auth::logout_cookies(config) {
            extra.push(("Set-Cookie".to_string(), cookie));
        }
        transfer::reset();
        return Err((302, format!("{}/#/", config.app_origin)));
    }

    if path == "/api/popular" {
        let response =
            api::invidious("/api/v1/popular", &[], config).map_err(|error| (502, error))?;
        let videos = mapping::clean_items(&as_array(&response), adblock);
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
        let videos = mapping::clean_items(&as_array(&response), adblock);
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
        let videos = mapping::clean_items(
            response
                .get("recommendedVideos")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            adblock,
        );
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
                "No progressive, DASH or HLS stream is available for this video.".to_string(),
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

    if path == "/api/sponsorblock" {
        let id = param(url, "id");
        if !mapping::valid_video_id(&id) {
            return Err((400, "Invalid video ID.".into()));
        }
        let segments = sponsorblock::get(&id, config).map_err(|error| (502, error))?;
        return Ok((200, segments));
    }

    if path == "/api/subscriptions" {
        let session = session.ok_or_else(|| (401, "Sign in to see subscriptions.".to_string()))?;
        let videos = api::subscription_videos(&session, config).map_err(|error| (502, error))?;
        return Ok((200, json!({ "videos": videos })));
    }

    if path == "/api/transfer/status" {
        return Ok((200, transfer::status()));
    }

    if path == "/api/transfer/result" {
        return Ok((200, transfer::result().map_err(|error| (500, error))?));
    }

    if path == "/api/transfer/start" {
        let session = session.ok_or_else(|| (401, "Sign in before transferring.".to_string()))?;
        transfer::start(&session, config).map_err(|error| (409, error))?;
        return Ok((200, transfer::status()));
    }

    if path == "/api/transfer/takeout" {
        let _session =
            session.ok_or_else(|| (401, "Sign in before importing Takeout.".to_string()))?;
        let payload: Value = serde_json::from_slice(body)
            .map_err(|error| format!("Invalid Takeout JSON: {error}"))
            .map_err(|error| (400, error))?;
        let imported = transfer::takeout(&payload).map_err(|error| (500, error))?;
        return Ok((200, imported));
    }

    if path == "/api/transfer/cancel" {
        transfer::cancel();
        return Ok((200, transfer::status()));
    }

    Err((404, "Unknown API endpoint.".into()))
}

fn dash_manifest(request: Request, url: &Url, config: &Config) {
    let id = param(url, "id");
    if !mapping::valid_video_id(&id) {
        json_reply(
            request,
            400,
            json!({ "error": "Invalid video ID." }),
            Vec::new(),
        );
        return;
    }
    let response = match api::invidious(&format!("/api/v1/videos/{id}"), &[], config) {
        Ok(value) => value,
        Err(error) => {
            json_reply(request, 502, json!({ "error": error }), Vec::new());
            return;
        }
    };
    let raw = response
        .get("dashUrl")
        .and_then(Value::as_str)
        .unwrap_or("");
    if raw.is_empty() {
        json_reply(
            request,
            404,
            json!({ "error": "This video has no DASH manifest from the metadata server." }),
            Vec::new(),
        );
        return;
    }
    let instance = config
        .invidious_instances
        .first()
        .map(String::as_str)
        .unwrap_or("");
    let manifest = match dash::absolute_manifest(raw, instance) {
        Ok(value) => value,
        Err(error) => {
            json_reply(request, 400, json!({ "error": error }), Vec::new());
            return;
        }
    };
    let fetched = match api::fetch(
        manifest.as_str(),
        Purpose::Media,
        config,
        None,
        MANIFEST_CAP,
    ) {
        Ok(value) => value,
        Err(error) => {
            json_reply(request, 502, json!({ "error": error }), Vec::new());
            return;
        }
    };
    if fetched.status != 200 {
        json_reply(
            request,
            502,
            json!({ "error": format!("Manifest host returned HTTP {}", fetched.status) }),
            Vec::new(),
        );
        return;
    }
    let mpd = String::from_utf8_lossy(&fetched.body);
    let rewritten = dash::rewrite(&mpd, &manifest, &id);
    reply(
        request,
        200,
        rewritten.into_bytes(),
        "application/dash+xml; charset=utf-8",
        vec![(
            "Cache-Control".to_string(),
            "public, max-age=300".to_string(),
        )],
    );
}

fn dash_segment(request: Request, url: &Url, config: &Config) {
    let id = param(url, "id");
    if !mapping::valid_video_id(&id) {
        json_reply(
            request,
            400,
            json!({ "error": "Invalid video ID." }),
            Vec::new(),
        );
        return;
    }
    let upstream = param(url, "u");
    if upstream.is_empty() {
        json_reply(
            request,
            400,
            json!({ "error": "Missing segment URL." }),
            Vec::new(),
        );
        return;
    }
    // dash.js normally sends a range for every segment. Missing ranges are
    // normalized to bytes=0- so initialization segments still work.
    let raw_range = request_header(&request, "Range").unwrap_or_else(|| "bytes=0-".into());
    let Some(range) = sane_range(&raw_range) else {
        json_reply(
            request,
            400,
            json!({ "error": "Invalid Range header." }),
            Vec::new(),
        );
        return;
    };
    let fetched = match api::fetch(&upstream, Purpose::Media, config, Some(&range), SEGMENT_CAP) {
        Ok(value) => value,
        Err(error) => {
            json_reply(request, 502, json!({ "error": error }), Vec::new());
            return;
        }
    };
    if fetched.status != 200 && fetched.status != 206 {
        json_reply(
            request,
            502,
            json!({ "error": format!("Segment host returned HTTP {}", fetched.status) }),
            Vec::new(),
        );
        return;
    }

    let mut extra = vec![
        ("Content-Length".to_string(), fetched.body.len().to_string()),
        (
            "Accept-Ranges".to_string(),
            fetched
                .accept_ranges
                .clone()
                .unwrap_or_else(|| "bytes".into()),
        ),
        (
            "Cache-Control".to_string(),
            "public, max-age=31536000, immutable".to_string(),
        ),
    ];
    if let Some(content_range) = fetched.content_range.as_deref() {
        extra.push(("Content-Range".to_string(), content_range.to_string()));
    } else if fetched.status == 206 {
        if let (Some(start), Some(total)) = (
            range.split_once('-').map(|(start, _)| start),
            fetched
                .content_length
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok()),
        ) {
            if let Ok(start) = start.parse::<u64>() {
                let end = start
                    .saturating_add(fetched.body.len() as u64)
                    .saturating_sub(1);
                extra.push((
                    "Content-Range".to_string(),
                    format!("bytes {start}-{end}/{total}"),
                ));
            }
        }
    }
    let status = if fetched.status == 206 { 206 } else { 200 };
    let content_type = if fetched.content_type.is_empty() {
        "video/mp4".to_string()
    } else {
        fetched.content_type
    };
    reply(request, status, fetched.body, &content_type, extra);
}

/// Accept only `bytes=start-end`, with a bounded length of 32 MiB.
fn sane_range(raw: &str) -> Option<String> {
    let value = raw.trim();
    let rest = value.strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    if start.is_empty()
        || !start.bytes().all(|byte| byte.is_ascii_digit())
        || end.bytes().any(|byte| !byte.is_ascii_digit())
    {
        return None;
    }
    let start: u64 = start.parse().ok()?;
    let end = if end.is_empty() {
        start.saturating_add(SEGMENT_CAP as u64 - 1)
    } else {
        end.parse::<u64>().ok()?
    };
    if end < start {
        return None;
    }
    Some(format!("bytes={start}-{end}"))
}

/// Same-origin image proxy. Metadata APIs provide the thumbnail URLs (as in
/// FreeTube); we validate and fetch them server-side so the browser never
/// contacts an image host directly.
fn image_proxy(request: Request, url: &Url, config: &Config) {
    let raw = param(url, "u");
    if raw.is_empty() {
        json_reply(
            request,
            400,
            json!({ "error": "Missing image URL." }),
            Vec::new(),
        );
        return;
    }
    // Relative API URLs are tried against every configured instance because
    // metadata requests fail over between instances.
    let locations: Vec<String> = if let Some(rest) = raw.strip_prefix("//") {
        vec![format!("https://{rest}")]
    } else if raw.starts_with('/') {
        if config.invidious_instances.is_empty() {
            json_reply(
                request,
                502,
                json!({ "error": "No Invidious instance configured." }),
                Vec::new(),
            );
            return;
        }
        let mut locations = Vec::new();
        let mut error = None;
        for instance in &config.invidious_instances {
            match Url::parse(instance).and_then(|base| base.join(&raw)) {
                Ok(resolved) => locations.push(resolved.to_string()),
                Err(failure) => error = Some(failure.to_string()),
            }
        }
        if locations.is_empty() {
            json_reply(
                request,
                400,
                json!({ "error": error.unwrap_or_default() }),
                Vec::new(),
            );
            return;
        }
        locations
    } else {
        vec![raw]
    };
    let mut last_error = "No image source available.".to_string();
    for location in locations {
        match api::fetch_binary(&location, Purpose::Image, config, THUMBNAIL_CAP) {
            Ok((body, content_type)) if content_type.starts_with("image/") => {
                reply(
                    request,
                    200,
                    body,
                    &content_type,
                    vec![(
                        "Cache-Control".to_string(),
                        "public, max-age=86400".to_string(),
                    )],
                );
                return;
            }
            Ok(_) => last_error = "Upstream did not return an image.".to_string(),
            Err(error) => last_error = error,
        }
    }
    json_reply(request, 502, json!({ "error": last_error }), Vec::new());
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
) -> Option<Result<Option<(String, Purpose, String, String)>, (u16, String)>> {
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
            Purpose::Thumbnail,
            "image/jpeg".to_string(),
            "public, max-age=86400, immutable".to_string(),
        ))));
    }

    if path == "/api/avatar" {
        let raw = param(url, "url");
        return Some(Ok(Some((
            raw,
            Purpose::Avatar,
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

fn read_body(request: &mut Request, max: usize) -> Vec<u8> {
    let reader = request.as_reader();
    let mut body = Vec::new();
    let mut buffer = [0u8; 8192];
    while body.len() < max {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => body.extend_from_slice(&buffer[..count]),
            Err(_) => break,
        }
    }
    body
}

fn json_reply(request: Request, status: u16, value: Value, extra: Vec<(String, String)>) {
    let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
    reply(
        request,
        status,
        body,
        "application/json; charset=utf-8",
        extra,
    );
}

fn static_reply(request: Request, path: &str, config: &Config) {
    // Hash routes are canonical, but allow direct links to the wizard pages.
    let route = match path {
        "/" | "/signin" | "/transfer" | "/settings" | "/watch" => "index.html",
        other => other.trim_start_matches('/'),
    };
    let file = config.public_dir.join(route);
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
            let cache = if canonical.extension().and_then(|value| value.to_str()) == Some("html") {
                "no-store"
            } else {
                "public, max-age=300"
            };
            reply(
                request,
                200,
                body,
                content_type,
                vec![("Cache-Control".to_string(), cache.to_string())],
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
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml; charset=utf-8",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_header_is_bounded() {
        assert_eq!(sane_range("bytes=0-1023"), Some("bytes=0-1023".into()));
        assert_eq!(
            sane_range("bytes=512-"),
            Some(format!("bytes=512-{}", 512 + SEGMENT_CAP as u64 - 1))
        );
        assert_eq!(sane_range("bytes=100-50"), None);
        assert_eq!(sane_range("bytes=abc-10"), None);
        assert_eq!(sane_range("items=0-1"), None);
    }

    #[test]
    fn csp_has_no_youtube_embed_dependency() {
        let csp = SECURITY_HEADERS[0].1;
        assert!(csp.contains("media-src 'self' blob:"));
        assert!(csp.contains("connect-src 'self' blob:"));
        assert!(!csp.contains("youtube"));
        assert!(!csp.contains("frame-src"));
    }
}
