use crate::auth::Session;
use crate::config::Config;
use serde_json::{Value, json};
use std::sync::OnceLock;
use ureq::tls::{TlsConfig, TlsProvider};
use url::Url;

const UA: &str = "SourceTube/1.1 (+privacy-first YouTube client)";
static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

pub fn agent() -> &'static ureq::Agent {
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .build(),
            )
            .build()
            .new_agent()
    })
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
        match agent()
            .get(url.as_str())
            .header("Accept", "application/json")
            .header("User-Agent", UA)
            .call()
        {
            Ok(mut response) => match response.body_mut().read_to_string() {
                Ok(text) => return serde_json::from_str(&text).map_err(|error| error.to_string()),
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

fn youtube(
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
    let mut request = agent()
        .get(url.as_str())
        .header("Accept", "application/json")
        .header("User-Agent", UA);
    if let Some(token) = access_token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|error| error.to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return Err(message.to_string());
    }
    Ok(value)
}

pub fn fetch_binary(url: &str) -> Result<(Vec<u8>, String), String> {
    let mut response = agent()
        .get(url)
        .header("User-Agent", UA)
        .call()
        .map_err(|error| error.to_string())?;
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| error.to_string())?;
    Ok((bytes, content_type))
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
