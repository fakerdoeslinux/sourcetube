use crate::auth::Session;
use crate::config::Config;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::OnceLock;
use ureq::tls::{TlsConfig, TlsProvider};
use url::Url;

const UA: &str = "SourceTube/1.1 (+privacy-first YouTube client)";
const JSON_BODY_LIMIT: u64 = 16 * 1024 * 1024;
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
        if !is_allowed_proxy_url(url.as_str(), config, &[]) {
            last_error = "Configured Invidious URL failed the outbound URL guard.".into();
            continue;
        }
        match agent()
            .get(url.as_str())
            .header("Accept", "application/json")
            .header("User-Agent", UA)
            .config()
            .max_redirects(0)
            .build()
            .call()
        {
            Ok(mut response) => match read_limited_text(response.body_mut(), JSON_BODY_LIMIT) {
                Ok(text) => return serde_json::from_str(&text).map_err(|error| error.to_string()),
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}

pub fn is_allowed_proxy_url(raw: &str, config: &Config, extra_hosts: &[&str]) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
    {
        return false;
    }
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let denied = config.adblock_enabled()
        && include_str!("../assets/adblock-rules.txt")
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("domain:"))
            .any(|blocked| host == blocked || host.ends_with(&format!(".{blocked}")));
    if host.is_empty() || denied {
        return false;
    }
    let configured = config.invidious_instances.iter().any(|instance| {
        Url::parse(instance)
            .ok()
            .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
            .is_some_and(|base| {
                host == base
                    || host.ends_with(&format!(".{base}"))
                    || is_instance_companion(&base, &host)
            })
    });
    let explicit = extra_hosts
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")));
    if !configured && !explicit {
        return false;
    }
    let port = url.port().unwrap_or(443);
    (host.as_str(), port)
        .to_socket_addrs()
        .is_ok_and(|addresses| {
            addresses
                .into_iter()
                .all(|address| !private_ip(address.ip()))
        })
}

fn is_instance_companion(instance_host: &str, host: &str) -> bool {
    let labels = instance_host.split('.').collect::<Vec<_>>();
    if labels.len() < 3 {
        return false;
    }
    let domain = labels[1..].join(".");
    let Some(prefix) = host.strip_suffix(&format!(".{domain}")) else {
        return false;
    };
    let Some(number) = prefix.strip_prefix("yt-comp") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 224
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ipv4| private_ip(IpAddr::V4(ipv4)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_instance_companion;

    #[test]
    fn recognizes_only_numeric_companion_hosts_on_the_instance_domain() {
        assert!(is_instance_companion(
            "yt.chocolatemoo53.com",
            "yt-comp4.chocolatemoo53.com"
        ));
        assert!(!is_instance_companion(
            "yt.chocolatemoo53.com",
            "yt-comp4.attacker.example"
        ));
        assert!(!is_instance_companion(
            "yt.chocolatemoo53.com",
            "yt-comp.attacker.chocolatemoo53.com"
        ));
        assert!(!is_instance_companion(
            "chocolatemoo53.com",
            "yt-comp4.chocolatemoo53.com"
        ));
    }
}

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
    let mut request = agent()
        .get(url.as_str())
        .header("Accept", "application/json")
        .header("User-Agent", UA);
    if let Some(token) = access_token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|error| error.to_string())?;
    let text = read_limited_text(response.body_mut(), JSON_BODY_LIMIT)?;
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

fn read_limited_text(body: &mut ureq::Body, limit: u64) -> Result<String, String> {
    let mut text = String::new();
    body.with_config()
        .limit(limit)
        .reader()
        .take(limit + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > limit {
        return Err(format!(
            "Upstream response exceeded the {limit}-byte limit."
        ));
    }
    Ok(text)
}

pub fn fetch_limited(
    url: &str,
    range: Option<&str>,
    limit: u64,
) -> Result<(u16, Vec<u8>, String, HashMap<String, String>), String> {
    let mut request = agent().get(url).header("User-Agent", UA);
    if let Some(range) = range {
        request = request
            .header("Range", range)
            .header("Accept-Encoding", "identity");
    }
    let mut response = request
        .config()
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect();
    let mut body = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .reader()
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|error| error.to_string())?;
    if body.len() as u64 > limit {
        return Err(format!(
            "Upstream response exceeded the {limit}-byte limit."
        ));
    }
    Ok((status, body, content_type, headers))
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
