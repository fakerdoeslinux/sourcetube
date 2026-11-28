use crate::auth::Session;
use crate::config::Config;
use crate::guard::{self, Purpose};
use serde_json::{Value, json};
use std::sync::OnceLock;
use ureq::tls::{TlsConfig, TlsProvider};
use url::Url;

const UA: &str = "SourceTube/1.1.3 (+privacy-first YouTube client)";
static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

pub fn agent() -> &'static ureq::Agent {
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .build(),
            )
            // Status codes are handled explicitly by fetch() so callers can
            // distinguish SponsorBlock 404 from a real upstream failure.
            .http_status_as_error(false)
            .build()
            .new_agent()
    })
}

#[derive(Debug)]
pub struct Fetch {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: String,
    pub content_range: Option<String>,
    pub accept_ranges: Option<String>,
    pub content_length: Option<String>,
}

fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// The single guarded outbound fetch used by every proxy endpoint.
pub fn fetch(
    raw: &str,
    purpose: Purpose,
    config: &Config,
    range: Option<&str>,
    max_bytes: usize,
) -> Result<Fetch, String> {
    let url = guard::validate(raw, purpose, config)?;
    let mut request = agent()
        .get(url.as_str())
        .header("Accept", "*/*")
        .header("User-Agent", UA);
    if let Some(range) = range {
        request = request.header("Range", range);
    }
    let mut response = request.call().map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let content_type =
        header(&response, "content-type").unwrap_or_else(|| "application/octet-stream".to_string());
    let content_range = header(&response, "content-range");
    let accept_ranges = header(&response, "accept-ranges");
    let content_length = header(&response, "content-length");
    let body = response
        .body_mut()
        .with_config()
        .limit(max_bytes as u64)
        .read_to_vec()
        .map_err(|error| format!("Upstream response exceeded the size cap: {error}"))?;
    if body.len() > max_bytes {
        return Err("Upstream response exceeded the size cap.".into());
    }
    Ok(Fetch {
        status,
        body,
        content_type,
        content_range,
        accept_ranges,
        content_length,
    })
}

pub fn fetch_json(
    raw: &str,
    purpose: Purpose,
    config: &Config,
    max_bytes: usize,
) -> Result<(u16, Value), String> {
    let fetched = fetch(raw, purpose, config, None, max_bytes)?;
    let text = String::from_utf8_lossy(&fetched.body);
    let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({}));
    Ok((fetched.status, value))
}

pub fn invidious(path: &str, params: &[(&str, &str)], config: &Config) -> Result<Value, String> {
    let mut last_error = String::from("no Invidious instance available");
    for instance in &config.invidious_instances {
        let mut url =
            Url::parse(&format!("{instance}{path}")).map_err(|error| error.to_string())?;
        if !params.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in params {
                pairs.append_pair(name, value);
            }
        }
        match fetch_json(url.as_str(), Purpose::Invidious, config, 8 * 1024 * 1024) {
            Ok((200, value)) => return Ok(value),
            Ok((status, _)) => last_error = format!("Invidious returned HTTP {status}"),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// YouTube Data API call with either an OAuth token or optional API key.
/// Public browsing never calls this function.
pub fn youtube(
    path: &str,
    params: &[(&str, &str)],
    access_token: Option<&str>,
    config: &Config,
) -> Result<Value, String> {
    let mut url = Url::parse(&format!("https://www.googleapis.com/youtube/v3/{path}"))
        .map_err(|error| error.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in params {
            pairs.append_pair(name, value);
        }
        if access_token.is_none() && !config.youtube_api_key.is_empty() {
            pairs.append_pair("key", &config.youtube_api_key);
        }
    }
    guard::validate(url.as_str(), Purpose::Google, config)?;
    let auth = access_token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Bearer {token}"));
    let mut request = agent()
        .get(url.as_str())
        .header("Accept", "application/json")
        .header("User-Agent", UA);
    if let Some(auth) = auth.as_deref() {
        request = request.header("Authorization", auth);
    }
    let response = request.call().map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response
        .into_body()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if status >= 400 {
        return Err(value
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("YouTube Data API returned HTTP {status}")));
    }
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return Err(message.to_string());
    }
    Ok(value)
}

pub fn fetch_binary(
    url: &str,
    purpose: Purpose,
    config: &Config,
    max_bytes: usize,
) -> Result<(Vec<u8>, String), String> {
    let fetched = fetch(url, purpose, config, None, max_bytes)?;
    if fetched.status != 200 {
        return Err(format!("Upstream returned HTTP {}", fetched.status));
    }
    Ok((fetched.body, fetched.content_type))
}

pub fn subscription_videos(session: &Session, config: &Config) -> Result<Vec<Value>, String> {
    let empty: Vec<Value> = Vec::new();
    let token = Some(session.access_token.as_str());
    let subs = youtube(
        "subscriptions",
        &[("part", "snippet"), ("mine", "true"), ("maxResults", "48")],
        token,
        config,
    )?;
    let channel_ids: Vec<String> = subs
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter_map(|item| {
            item.get("snippet")?
                .get("channelId")?
                .as_str()
                .map(str::to_string)
        })
        .take(12)
        .collect();
    if channel_ids.is_empty() {
        return Ok(Vec::new());
    }

    let channels = youtube(
        "channels",
        &[
            ("part", "contentDetails"),
            ("id", &channel_ids.join(",")),
            ("maxResults", "50"),
        ],
        token,
        config,
    )?;
    let playlists: Vec<String> = channels
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter_map(|item| {
            item.get("contentDetails")?
                .get("relatedPlaylists")?
                .get("uploads")?
                .as_str()
                .map(str::to_string)
        })
        .take(12)
        .collect();

    let mut ids = Vec::new();
    for playlist in playlists {
        let items = youtube(
            "playlistItems",
            &[
                ("part", "snippet"),
                ("playlistId", &playlist),
                ("maxResults", "4"),
            ],
            token,
            config,
        )
        .unwrap_or_else(|_| json!({ "items": [] }));
        for item in items
            .get("items")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            if let Some(id) = item
                .get("snippet")
                .and_then(|snippet| snippet.get("resourceId"))
                .and_then(|resource| resource.get("videoId"))
                .and_then(Value::as_str)
            {
                ids.push(id.to_string());
            }
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let details = youtube(
        "videos",
        &[
            ("part", "snippet,statistics,contentDetails"),
            ("id", &ids.join(",")),
        ],
        token,
        config,
    )?;
    let mut videos: Vec<Value> = details
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .map(crate::mapping::map_official_video)
        .collect();
    videos.sort_by(|a, b| {
        let a_time = a.get("publishedAt").and_then(Value::as_str).unwrap_or("");
        let b_time = b.get("publishedAt").and_then(Value::as_str).unwrap_or("");
        b_time.cmp(a_time)
    });
    Ok(videos)
}
