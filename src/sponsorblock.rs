//! Server-side SponsorBlock proxy with a small bounded TTL cache.

use crate::{api, config::Config, guard::Purpose};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_ENTRIES: usize = 256;
const MAX_BODY: usize = 1024 * 1024;

struct Cache {
    entries: HashMap<String, (Instant, Value)>,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: HashMap::new(),
        })
    })
}

pub fn segment_url(id: &str) -> String {
    format!(
        "https://sponsor.ajay.app/api/skipSegments?videoID={id}&category=sponsor&category=intro&category=outro&category=selfpromo&category=interaction&category=music_offtopic&category=poi_highlight"
    )
}

pub fn get(id: &str, config: &Config) -> Result<Value, String> {
    if let Ok(mut cache) = cache().lock() {
        let now = Instant::now();
        cache
            .entries
            .retain(|_, (stored, _)| now.duration_since(*stored) < TTL);
        if let Some((_, value)) = cache.entries.get(id) {
            return Ok(value.clone());
        }
    }

    let (status, value) =
        api::fetch_json(&segment_url(id), Purpose::SponsorBlock, config, MAX_BODY)?;
    let segments: Value = if status == 404 {
        json!([])
    } else if status != 200 {
        return Err(format!("SponsorBlock returned HTTP {status}"));
    } else if let Some(array) = value.as_array() {
        Value::Array(array.clone())
    } else {
        json!([])
    };
    let result = json!({ "segments": segments, "source": "proxied" });

    if let Ok(mut cache) = cache().lock() {
        let now = Instant::now();
        cache
            .entries
            .retain(|_, (stored, _)| now.duration_since(*stored) < TTL);
        if cache.entries.len() >= MAX_ENTRIES {
            let oldest = cache
                .entries
                .iter()
                .min_by_key(|(_, (stored, _))| *stored)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                cache.entries.remove(&oldest);
            }
        }
        cache.entries.insert(id.to_string(), (now, result.clone()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_has_all_categories() {
        let url = segment_url("abcdefghijk");
        for category in [
            "sponsor",
            "intro",
            "outro",
            "selfpromo",
            "interaction",
            "music_offtopic",
            "poi_highlight",
        ] {
            assert!(url.contains(&format!("category={category}")));
        }
    }
}
