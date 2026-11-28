use crate::api;
use crate::auth::{self, Session};
use crate::config::Config;
use crate::mapping;
use crate::transfer;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tiny_http::{Header, Request, Response};
use url::Url;

const DASH_BODY_LIMIT: u64 = 32 * 1024 * 1024;
const TAKEOUT_BODY_LIMIT: u64 = 16 * 1024 * 1024;
static SPONSOR_CACHE: OnceLock<Mutex<HashMap<String, (Instant, Value)>>> = OnceLock::new();

fn sane_range(range: &str) -> bool {
    let Some(value) = range.strip_prefix("bytes=") else {
        return false;
    };
    if value.contains(',') {
        return false;
    }
    let Some((start, end)) = value.split_once('-') else {
        return false;
    };
    let Ok(start) = start.parse::<u64>() else {
        return false;
    };
    if end.is_empty() {
        return true;
    }
    let Ok(end) = end.parse::<u64>() else {
        return false;
    };
    end >= start && end.saturating_sub(start) < DASH_BODY_LIMIT
}
const SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self' blob:; font-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
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

pub fn handle(mut request: Request, config: &Config) {
    let host = request_header(&request, "Host").unwrap_or_else(|| "localhost".to_string());
    let url = Url::parse(&format!("http://{host}{}", request.url()))
        .unwrap_or_else(|_| Url::parse("http://localhost/").unwrap());
    let path = url.path().to_string();

    if path == "/api/transfer/takeout" && request.method().as_str() == "POST" {
        let cookies = auth::parse_cookies(request_header(&request, "Cookie").as_deref());
        if auth::load_session(&cookies, config).0.is_none() {
            json_reply(
                request,
                401,
                json!({ "error": "Sign in before importing Takeout data." }),
                Vec::new(),
            );
            return;
        }
        let mut body = Vec::new();
        let result = request
            .as_reader()
            .take(TAKEOUT_BODY_LIMIT + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())
            .and_then(|_| {
                if body.len() as u64 > TAKEOUT_BODY_LIMIT {
                    return Err("Takeout JSON exceeds the 16 MiB upload limit.".into());
                }
                let payload: Value =
                    serde_json::from_slice(&body).map_err(|error| error.to_string())?;
                let filename = payload
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let data = payload
                    .get("data")
                    .ok_or_else(|| "Missing Takeout JSON data.".to_string())?;
                transfer::import_takeout(filename, data, &config.data_dir)
            });
        match result {
            Ok(imported) => json_reply(request, 200, json!({ "imported": imported }), Vec::new()),
            Err(error) => json_reply(request, 400, json!({ "error": error }), Vec::new()),
        }
        return;
    }

    if path == "/api/settings" && request.method().as_str() == "POST" {
        let mut body = String::new();
        let result = request
            .as_reader()
            .take(4096)
            .read_to_string(&mut body)
            .map_err(|error| error.to_string())
            .and_then(|_| {
                let value: Value =
                    serde_json::from_str(&body).map_err(|error| error.to_string())?;
                let enabled = value
                    .get("adblockEnabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| "adblockEnabled must be a boolean.".to_string())?;
                config.set_adblock_enabled(enabled)?;
                Ok(enabled)
            });
        match result {
            Ok(enabled) => json_reply(
                request,
                200,
                json!({ "adblockEnabled": enabled }),
                Vec::new(),
            ),
            Err(error) => json_reply(request, 400, json!({ "error": error }), Vec::new()),
        }
        return;
    }

    if path.starts_with("/api/") {
        if path == "/api/dash/manifest" {
            dash_manifest(request, &url, config);
            return;
        }
        if path == "/api/dash/segment" {
            dash_segment(request, &url, config);
            return;
        }
        if path == "/api/sponsorblock" {
            let id = param(&url, "id");
            let result = if mapping::valid_video_id(&id) {
                sponsor_segments(&id, config)
            } else {
                Err((400, "Invalid video ID.".into()))
            };
            match result {
                Ok(segments) => {
                    json_reply(request, 200, json!({ "segments": segments }), Vec::new())
                }
                Err((status, error)) => {
                    json_reply(request, status, json!({ "error": error }), Vec::new())
                }
            }
            return;
        }
        if let Some(source) = binary_source(&path, &url, config) {
            match source {
                Ok(Some((location, content_type, cache))) => {
                    if !proxy_url_allowed(&location, config, ProxyPurpose::Images) {
                        json_reply(
                            request,
                            403,
                            json!({ "error": "The image host failed the outbound URL guard." }),
                            Vec::new(),
                        );
                        return;
                    }
                    match fetch_proxy_limited(
                        &location,
                        None,
                        8 * 1024 * 1024,
                        config,
                        ProxyPurpose::Images,
                    ) {
                        Ok((_, status, body, upstream_type, _)) if (200..300).contains(&status) => {
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
                        Ok((_, status, _, _, _)) => {
                            json_reply(
                                request,
                                502,
                                json!({ "error": format!("Image host returned HTTP {status}.") }),
                                Vec::new(),
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
            let Some(raw_manifest) = response.get("dashUrl").and_then(Value::as_str) else {
                json_reply(
                    request,
                    404,
                    json!({ "error": "No DASH manifest is available for this video." }),
                    Vec::new(),
                );
                return;
            };
            let Some(instance) = config.invidious_instances.first() else {
                json_reply(
                    request,
                    502,
                    json!({ "error": "No Invidious instance is configured." }),
                    Vec::new(),
                );
                return;
            };
            let manifest_url = if raw_manifest.starts_with('/') {
                format!("{instance}{raw_manifest}")
            } else {
                raw_manifest.to_string()
            };
            if !proxy_url_allowed(&manifest_url, config, ProxyPurpose::Invidious) {
                json_reply(
                    request,
                    502,
                    json!({ "error": "The DASH manifest host is not allowed." }),
                    Vec::new(),
                );
                return;
            }

            match fetch_proxy_limited(
                &manifest_url,
                None,
                DASH_BODY_LIMIT,
                config,
                ProxyPurpose::Dash,
            ) {
                Ok((final_url, status, body, _, _)) if (200..300).contains(&status) => {
                    let Ok(mut manifest) = String::from_utf8(body) else {
                        json_reply(
                            request,
                            502,
                            json!({ "error": "The DASH manifest was not valid UTF-8." }),
                            Vec::new(),
                        );
                        return;
                    };
                    rewrite_dash_manifest(&mut manifest, &final_url, &id, config);
                    reply(
                        request,
                        200,
                        manifest.into_bytes(),
                        "application/dash+xml; charset=utf-8",
                        vec![("Cache-Control".to_string(), "no-store".to_string())],
                    );
                }
                Ok((_, status, _, _, _)) => json_reply(
                    request,
                    502,
                    json!({ "error": format!("Upstream DASH manifest returned HTTP {status}.") }),
                    Vec::new(),
                ),
                Err(error) => json_reply(request, 502, json!({ "error": error }), Vec::new()),
            }
        }

        fn rewrite_dash_manifest(
            manifest: &mut String,
            manifest_url: &str,
            id: &str,
            config: &Config,
        ) {
            let base_for_relative = manifest
                .find("<BaseURL")
                .and_then(|start| {
                    let open_end = manifest[start..].find('>')? + start;
                    let close = manifest[open_end + 1..].find("</BaseURL>")? + open_end + 1;
                    let raw = manifest[open_end + 1..close].trim().replace("&amp;", "&");
                    Url::parse(&raw)
                        .or_else(|_| Url::parse(manifest_url)?.join(&raw))
                        .ok()
                })
                .or_else(|| Url::parse(manifest_url).ok());
            for (tag, attribute) in [
                ("<BaseURL", None),
                ("<SegmentURL", Some("media")),
                ("<Initialization", Some("sourceURL")),
                ("<SegmentTemplate", Some("media")),
                ("<SegmentTemplate", Some("initialization")),
                ("<Representation", Some("media")),
            ] {
                let mut cursor = 0;
                while let Some(relative_start) = manifest[cursor..].find(tag) {
                    let start = cursor + relative_start;
                    let Some(relative_end) = manifest[start..].find('>') else {
                        break;
                    };
                    let end = start + relative_end;
                    if let Some(attribute) = attribute {
                        let Some(value_start) =
                            manifest[start..end].find(&format!("{attribute}=\""))
                        else {
                            cursor = end + 1;
                            continue;
                        };
                        let value_start = start + value_start + attribute.len() + 2;
                        let Some(value_end) = manifest[value_start..end].find('"') else {
                            cursor = end + 1;
                            continue;
                        };
                        let value_end = value_start + value_end;
                        let raw = manifest[value_start..value_end].replace("&amp;", "&");
                        if let Some(base) = &base_for_relative
                            && let Ok(upstream) = Url::parse(&raw).or_else(|_| base.join(&raw))
                        {
                            if proxy_url_allowed(upstream.as_str(), config, ProxyPurpose::Dash) {
                                let replacement =
                                    dash_proxy_url(id, upstream.as_str()).replace('&', "&amp;");
                                manifest.replace_range(value_start..value_end, &replacement);
                                cursor = value_start + replacement.len();
                            } else {
                                cursor = end + 1;
                            }
                        } else {
                            cursor = end + 1;
                        }
                    } else {
                        let close = manifest[end + 1..].find("</BaseURL>");
                        if let Some(close) = close {
                            let value_start = end + 1;
                            let value_end = value_start + close;
                            let raw = manifest[value_start..value_end]
                                .trim()
                                .replace("&amp;", "&");
                            if let Some(base) = &base_for_relative
                                && let Ok(upstream) = Url::parse(&raw).or_else(|_| base.join(&raw))
                            {
                                if proxy_url_allowed(upstream.as_str(), config, ProxyPurpose::Dash)
                                {
                                    let replacement = format!(
                                        "<BaseURL>{}</BaseURL>",
                                        dash_proxy_url(id, upstream.as_str()).replace('&', "&amp;")
                                    );
                                    manifest.replace_range(
                                        start..value_end + "</BaseURL>".len(),
                                        &replacement,
                                    );
                                    cursor = start + replacement.len();
                                } else {
                                    cursor = value_end + "</BaseURL>".len();
                                }
                            } else {
                                cursor = value_end + "</BaseURL>".len();
                            }
                        } else {
                            cursor = end + 1;
                        }
                    }
                }
            }
        }

        fn dash_proxy_url(id: &str, upstream: &str) -> String {
            let mut encoded =
                url::form_urlencoded::byte_serialize(upstream.as_bytes()).collect::<String>();
            encoded = encoded.replace("%24", "$");
            format!("/api/dash/segment?id={id}&u={encoded}")
        }

        fn dash_segment(request: Request, url: &Url, config: &Config) {
            let id = param(url, "id");
            let upstream = param(url, "u");
            let range = request_header(&request, "Range").unwrap_or_default();
            if !mapping::valid_video_id(&id) || !sane_range(&range) {
                json_reply(
                    request,
                    400,
                    json!({ "error": "A valid video ID and byte Range are required." }),
                    Vec::new(),
                );
                return;
            }
            if !proxy_url_allowed(&upstream, config, ProxyPurpose::Dash) {
                json_reply(
                    request,
                    403,
                    json!({ "error": "The media host is not allowed." }),
                    Vec::new(),
                );
                return;
            }
            match fetch_proxy_limited(
                &upstream,
                Some(&range),
                DASH_BODY_LIMIT,
                config,
                ProxyPurpose::Dash,
            ) {
                Ok((_, 206, body, content_type, headers)) => {
                    let mut extra = vec![
                        ("Accept-Ranges".to_string(), "bytes".to_string()),
                        ("Cache-Control".to_string(), "no-store".to_string()),
                    ];
                    for name in ["Content-Range", "Content-Length", "Accept-Ranges"] {
                        if let Some(value) = headers.get(name.to_ascii_lowercase().as_str()) {
                            extra.push((name.to_string(), value.clone()));
                        }
                    }
                    reply(request, 206, body, &content_type, extra);
                }
                Ok((_, status, _, _, _)) => json_reply(
                    request,
                    502,
                    json!({ "error": format!("Upstream media returned HTTP {status}; expected 206.") }),
                    Vec::new(),
                ),
                Err(error) => json_reply(request, 502, json!({ "error": error }), Vec::new()),
            }
        }

        #[derive(Clone, Copy)]
        enum ProxyPurpose {
            Invidious,
            Dash,
            SponsorBlock,
            Images,
        }

        fn proxy_url_allowed(raw: &str, config: &Config, purpose: ProxyPurpose) -> bool {
            let extra_hosts: &[&str] = match purpose {
                ProxyPurpose::Invidious => &[],
                ProxyPurpose::Dash => &["googlevideo.com"],
                ProxyPurpose::SponsorBlock => &["sponsor.ajay.app"],
                ProxyPurpose::Images => &["i.ytimg.com", "googleusercontent.com"],
            };
            api::is_allowed_proxy_url(raw, config, extra_hosts)
        }

        fn fetch_proxy_limited(
            raw: &str,
            range: Option<&str>,
            limit: u64,
            config: &Config,
            purpose: ProxyPurpose,
        ) -> Result<
            (
                String,
                u16,
                Vec<u8>,
                String,
                std::collections::HashMap<String, String>,
            ),
            String,
        > {
            let mut current =
                Url::parse(raw).map_err(|error| format!("Invalid upstream URL: {error}"))?;
            for _ in 0..=5 {
                if !proxy_url_allowed(current.as_str(), config, purpose) {
                    return Err("Redirect target failed the outbound URL guard.".into());
                }
                let (status, body, content_type, headers) =
                    api::fetch_limited(current.as_str(), range, limit)?;
                if !matches!(status, 301 | 302 | 303 | 307 | 308) {
                    return Ok((current.to_string(), status, body, content_type, headers));
                }
                let location = headers.get("location").ok_or_else(|| {
                    "Upstream redirect did not include a Location header.".to_string()
                })?;
                current = current
                    .join(location)
                    .map_err(|error| format!("Invalid upstream redirect URL: {error}"))?;
            }
            Err("Upstream exceeded the redirect limit.".into())
        }

        fn sponsor_segments(id: &str, config: &Config) -> Result<Value, (u16, String)> {
            let cache = SPONSOR_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
            if let Ok(mut entries) = cache.lock() {
                entries.retain(|_, (stored, _)| stored.elapsed() < Duration::from_secs(600));
                if let Some((_, value)) = entries.get(id) {
                    return Ok(value.clone());
                }
            }

            let mut upstream = Url::parse("https://sponsor.ajay.app/api/skipSegments")
                .map_err(|error| (500, error.to_string()))?;
            {
                let mut query = upstream.query_pairs_mut();
                query.append_pair("videoID", id);
                for category in [
                    "sponsor",
                    "intro",
                    "outro",
                    "selfpromo",
                    "interaction",
                    "music_offtopic",
                    "poi_highlight",
                ] {
                    query.append_pair("category", category);
                }
            }
            if !proxy_url_allowed(upstream.as_str(), config, ProxyPurpose::SponsorBlock) {
                return Err((
                    502,
                    "SponsorBlock host failed the outbound URL guard.".into(),
                ));
            }
            let (_, status, body, _, _) = fetch_proxy_limited(
                upstream.as_str(),
                None,
                1024 * 1024,
                config,
                ProxyPurpose::SponsorBlock,
            )
            .map_err(|error| (502, error))?;
            let segments = if status == 404 {
                json!([])
            } else if (200..300).contains(&status) {
                serde_json::from_slice::<Value>(&body).map_err(|error| (502, error.to_string()))?
            } else {
                return Err((502, format!("SponsorBlock returned HTTP {status}.")));
            };
            if !segments.is_array() {
                return Err((502, "SponsorBlock returned an invalid response.".into()));
            }

            if let Ok(mut entries) = cache.lock() {
                if entries.len() >= 256
                    && let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, (stored, _))| *stored)
                        .map(|(key, _)| key.clone())
                {
                    entries.remove(&oldest);
                }
                entries.insert(id.to_string(), (Instant::now(), segments.clone()));
            }
            Ok(segments)
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
                "adblockEnabled": config.adblock_enabled(),
                "metadataApi": "FreeTube-compatible Invidious",
                "privacy": {
                    "analytics": false,
                    "trackers": false,
                    "clientIpToGoogleForMetadata": false
                }
            }),
        ));
    }

    if path == "/api/settings" {
        return Ok((200, json!({ "adblockEnabled": config.adblock_enabled() })));
    }

    if path == "/api/transfer/status" {
        return Ok((
            200,
            serde_json::to_value(transfer::current_status()).unwrap_or_else(|_| json!({})),
        ));
    }

    if path == "/api/transfer/start" {
        let session =
            session.ok_or_else(|| (401, "Sign in before starting a transfer.".to_string()))?;
        transfer::start(session, config.clone()).map_err(|error| (409, error))?;
        return Ok((202, json!({ "started": true })));
    }

    if path == "/api/transfer/cancel" {
        if session.is_none() {
            return Err((401, "Sign in before cancelling a transfer.".into()));
        }
        transfer::cancel();
        return Ok((200, json!({ "cancelled": true })));
    }

    if path == "/api/transfer/result" {
        if session.is_none() {
            return Err((401, "Sign in to view imported records.".into()));
        }
        let result = transfer::results(&config.data_dir).map_err(|error| (500, error))?;
        return Ok((200, result));
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
            .filter(|item| {
                item.get("videoId").is_some()
                    && !mapping::blocked_channel(item, config.adblock_enabled())
            })
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
            .filter(|item| {
                item.get("videoId").is_some()
                    && !mapping::blocked_channel(item, config.adblock_enabled())
            })
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
            .filter(|item| !mapping::blocked_channel(item, config.adblock_enabled()))
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
        let videos = api::subscription_videos(&session, config)
            .map_err(|error| (502, error))?
            .into_iter()
            .filter(|video| !mapping::blocked_channel(video, config.adblock_enabled()))
            .collect::<Vec<_>>();
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
    let relative = if path == "/" || path == "/signin" || path == "/transfer" {
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

#[cfg(test)]
mod tests {
    use super::sane_range;

    #[test]
    fn accepts_bounded_single_byte_ranges() {
        assert!(sane_range("bytes=0-"));
        assert!(sane_range("bytes=100-199"));
        assert!(sane_range("bytes=0-33554431"));
    }

    #[test]
    fn rejects_invalid_or_oversized_ranges() {
        assert!(!sane_range(""));
        assert!(!sane_range("bytes=-500"));
        assert!(!sane_range("bytes=200-100"));
        assert!(!sane_range("bytes=0-33554432"));
        assert!(!sane_range("bytes=0-10,20-30"));
    }
}
