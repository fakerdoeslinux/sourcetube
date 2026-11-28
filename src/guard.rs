//! Outbound URL guard and bundled privacy rules.
//!
//! Every server-side fetch goes through [`validate`]. Unknown hosts, plain
//! HTTP, private addresses and hosts matched by `assets/adblock-rules.txt`
//! are rejected before a connection is opened.

use crate::config::Config;
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::OnceLock;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Invidious,
    Thumbnail,
    Image,
    Avatar,
    Media,
    SponsorBlock,
    Google,
}

#[derive(Default)]
struct Rules {
    hosts: Vec<String>,
    channels: HashSet<String>,
}

static RULES: OnceLock<Rules> = OnceLock::new();

fn rules() -> &'static Rules {
    RULES.get_or_init(|| {
        let mut rules = Rules::default();
        let mut sources = Vec::new();
        // The canonical file lives in assets/; a copy is shipped in public/
        // for packaged installs where only the public directory is writable.
        sources.push(String::from(include_str!("../assets/adblock-rules.txt")));
        if let Ok(path) = std::env::var("SOURCETUBE_PUBLIC") {
            sources.push(
                std::fs::read_to_string(std::path::Path::new(&path).join("adblock-rules.txt"))
                    .unwrap_or_default(),
            );
        }
        for source in sources {
            for line in source.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(host) = line
                    .strip_prefix("||")
                    .and_then(|value| value.strip_suffix('^'))
                {
                    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
                    if !host.is_empty() && !rules.hosts.contains(&host) {
                        rules.hosts.push(host);
                    }
                } else if let Some(channel) = line.strip_prefix("channel:") {
                    rules.channels.insert(channel.trim().to_string());
                }
            }
        }
        rules
    })
}

pub fn is_blocked_channel(id: &str) -> bool {
    rules().channels.contains(id)
}

fn host_blocked(host: &str) -> bool {
    rules()
        .hosts
        .iter()
        .any(|rule| host == rule || host.ends_with(&format!(".{rule}")))
}

/// Validate an upstream URL for a specific endpoint before any network I/O.
pub fn validate(raw: &str, purpose: Purpose, config: &Config) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid upstream URL.".to_string())?;
    if url.scheme() != "https" {
        return Err("Only HTTPS upstream URLs are allowed.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Upstream URLs with credentials are not allowed.".into());
    }
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() || host_blocked(&host) {
        return Err("Upstream host is blocked by the bundled privacy rules.".into());
    }
    if matches!(host.as_str(), "localhost" | "localhost.localdomain")
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err("Local upstream hosts are not allowed.".into());
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        if !is_public_address(address) {
            return Err("Private or link-local upstream addresses are not allowed.".into());
        }
    }

    let allowed = match purpose {
        Purpose::Invidious => config
            .invidious_instances
            .iter()
            .any(|instance| host_matches(instance, &host)),
        Purpose::Thumbnail => host == "i.ytimg.com",
        // FreeTube-style image proxying: thumbnails come from the configured
        // Invidious instance (or Google's image hosts), never from arbitrary
        // hosts, and are always fetched server-side.
        Purpose::Image => {
            host == "i.ytimg.com"
                || host == "yt3.ggpht.com"
                || host == "yt3.googleusercontent.com"
                || host.ends_with(".googleusercontent.com")
                || config
                    .invidious_instances
                    .iter()
                    .any(|instance| host_matches(instance, &host))
        }
        Purpose::Avatar => {
            host == "googleusercontent.com" || host.ends_with(".googleusercontent.com")
        }
        Purpose::Media => {
            host == "googlevideo.com"
                || host.ends_with(".googlevideo.com")
                || config
                    .invidious_instances
                    .iter()
                    .any(|instance| host_matches(instance, &host))
                || config
                    .allowed_hosts
                    .iter()
                    .any(|value| host == *value || host.ends_with(&format!(".{value}")))
        }
        Purpose::SponsorBlock => host == "sponsor.ajay.app",
        Purpose::Google => matches!(
            host.as_str(),
            "accounts.google.com" | "oauth2.googleapis.com" | "www.googleapis.com"
        ),
    };
    if !allowed {
        return Err(format!(
            "Upstream host '{host}' is not allowed for this endpoint."
        ));
    }
    Ok(url)
}

/// Conservative public-address check (IpAddr::is_global is still unstable).
fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            let private = matches!(a, 0 | 10 | 127 | 224..=255)
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 100 && (64..=127).contains(&b))
                || (a == 203 && b == 0 && c == 113);
            !private
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4() {
                return is_public_address(IpAddr::V4(v4));
            }
            let segments = ip.segments();
            !matches!(segments[0], 0 | 0xfc00 | 0xfd00)
                && !(0xfe80..=0xfebf).contains(&segments[0])
                && segments[0] & 0xff00 != 0xff00
        }
    }
}

fn host_matches(instance: &str, host: &str) -> bool {
    Url::parse(instance)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|candidate| candidate == host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            port: 8787,
            app_origin: "http://localhost:8787".into(),
            redirect_uri: "http://localhost:8787/api/auth/callback".into(),
            google_client_id: String::new(),
            google_client_secret: String::new(),
            youtube_api_key: String::new(),
            session_secret: [0; 32],
            invidious_instances: vec!["https://yt.chocolatemoo53.com".into()],
            allowed_hosts: vec!["companion.example.net".into()],
            public_dir: std::path::PathBuf::from("public"),
            strict_port: false,
            port_in_use_fallback: false,
            secure_cookies: false,
        }
    }

    #[test]
    fn rejects_non_https_and_local_hosts() {
        let config = config();
        assert!(
            validate(
                "http://yt.chocolatemoo53.com/x",
                Purpose::Invidious,
                &config
            )
            .is_err()
        );
        assert!(validate("https://127.0.0.1/x", Purpose::Media, &config).is_err());
        assert!(validate("https://169.254.169.254/latest", Purpose::Media, &config).is_err());
        assert!(validate("https://example.local/x", Purpose::Media, &config).is_err());
    }

    #[test]
    fn enforces_endpoint_allowlists() {
        let config = config();
        assert!(
            validate(
                "https://yt.chocolatemoo53.com/api/v1/popular",
                Purpose::Invidious,
                &config
            )
            .is_ok()
        );
        assert!(
            validate(
                "https://rr1---sn.example.googlevideo.com/videoplayback",
                Purpose::Media,
                &config
            )
            .is_ok()
        );
        assert!(
            validate(
                "https://companion.example.net/segment",
                Purpose::Media,
                &config
            )
            .is_ok()
        );
        assert!(validate("https://evil.example/segment", Purpose::Media, &config).is_err());
        assert!(
            validate(
                "https://yt.chocolatemoo53.com/segment",
                Purpose::SponsorBlock,
                &config
            )
            .is_err()
        );
        assert!(
            validate(
                "https://sponsor.ajay.app/api/skipSegments",
                Purpose::SponsorBlock,
                &config
            )
            .is_ok()
        );
    }

    #[test]
    fn image_proxy_allows_only_known_image_hosts() {
        let config = config();
        assert!(
            validate(
                "https://yt.chocolatemoo53.com/vi/abcdefghijk/maxres.jpg",
                Purpose::Image,
                &config
            )
            .is_ok()
        );
        assert!(
            validate(
                "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg",
                Purpose::Image,
                &config
            )
            .is_ok()
        );
        assert!(validate("https://evil.example/tracker.png", Purpose::Image, &config).is_err());
    }

    #[test]
    fn blocks_bundled_ad_hosts() {
        let config = config();
        assert!(validate("https://doubleclick.net/ad", Purpose::Media, &config).is_err());
        assert!(
            validate(
                "https://www.google-analytics.com/collect",
                Purpose::Media,
                &config
            )
            .is_err()
        );
        assert!(is_blocked_channel("UCadsample00000000000000"));
    }
}
