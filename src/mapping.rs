use serde_json::{Value, json};
use url::Url;

pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Wrap an upstream image URL in the same-origin proxy. The browser never
/// requests third-party images directly (CSP allows only `self` and `data:`).
pub fn image_proxy_url(raw: &str) -> String {
    let mut local = Url::parse("http://sourcetube.local/api/image").expect("valid base");
    local.query_pairs_mut().append_pair("u", raw);
    format!(
        "{}{}",
        local.path(),
        local
            .query()
            .map(|query| format!("?{query}"))
            .unwrap_or_default()
    )
}

fn thumbnail_score(quality: &str, width: u64, height: u64) -> u64 {
    // FreeTube uses the 16:9 medium thumbnail for video lists. Prefer it over
    // the letterboxed 4:3 high thumbnail and over maxres images that YouTube
    // often replaces with a placeholder when no 1280x720 image exists.
    let quality = match quality {
        "medium" | "mqdefault" => 5,
        "high" | "hqdefault" | "sd" | "sddefault" => 3,
        "maxres" | "maxresdefault" => 2,
        "default" => 1,
        _ => 0,
    };
    let widescreen = u64::from(width > 0 && height > 0 && width * 9 >= height * 16);
    quality * 4 + widescreen * 2 + u64::from(width >= 320)
}

fn usable_image_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if raw.starts_with("https://") || raw.starts_with('/') {
        return Some(raw.to_string());
    }
    None
}

/// Choose the best thumbnail offered by the metadata API, following FreeTube's
/// approach of using the API-provided URLs instead of guessing one. Relative
/// URLs are resolved by `/api/image` against the configured Invidious instance.
pub fn thumbnail(value: &Value) -> String {
    let mut best: Option<(u64, String)> = None;
    let mut consider = |quality: &str, item: &Value| {
        let Some(raw) = item.get("url").and_then(Value::as_str) else {
            return;
        };
        let Some(url) = usable_image_url(raw) else {
            return;
        };
        let width = item.get("width").and_then(Value::as_u64).unwrap_or(0);
        let height = item.get("height").and_then(Value::as_u64).unwrap_or(0);
        let score = thumbnail_score(quality, width, height);
        if best.as_ref().is_none_or(|(current, _)| score > *current) {
            best = Some((score, url));
        }
    };

    if let Some(list) = value.get("videoThumbnails").and_then(Value::as_array) {
        for item in list {
            consider(&text(item, "quality"), item);
        }
    }
    if let Some(entries) = value
        .get("snippet")
        .and_then(|snippet| snippet.get("thumbnails"))
        .and_then(Value::as_object)
    {
        for (quality, item) in entries {
            consider(quality, item);
        }
    }
    if let Some((_, url)) = best {
        return image_proxy_url(&url);
    }

    let id = text(value, "videoId");
    let id = if id.is_empty() { text(value, "id") } else { id };
    if valid_video_id(&id) {
        format!("/api/thumb/{id}/hqdefault.jpg")
    } else {
        "/icon-192.png".to_string()
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn published(value: &Value) -> Value {
    if let Some(stamp) = value.get("published").and_then(Value::as_u64) {
        json!(stamp)
    } else {
        json!(text(value, "publishedText"))
    }
}

pub fn map_video(value: &Value) -> Value {
    let id = text(value, "videoId");
    let likes = value
        .get("likedCount")
        .and_then(Value::as_u64)
        .or_else(|| value.get("likes").and_then(Value::as_u64))
        .unwrap_or(0);

    json!({
        "id": id,
        "title": text(value, "title"),
        "channel": text(value, "author"),
        "channelId": text(value, "authorId"),
        "description": text(value, "description"),
        "publishedAt": published(value),
        "thumbnail": thumbnail(value),
        "views": number(value, "viewCount"),
        "likes": likes,
        "duration": number(value, "lengthSeconds")
    })
}

pub fn map_official_video(item: &Value) -> Value {
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let snippet = item.get("snippet").unwrap_or(&Value::Null);
    let stats = item.get("statistics").unwrap_or(&Value::Null);
    let details = item.get("contentDetails").unwrap_or(&Value::Null);
    let views = stats
        .get("viewCount")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let likes = stats
        .get("likeCount")
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    json!({
        "id": id,
        "title": snippet.get("title").and_then(Value::as_str).unwrap_or("Untitled video"),
        "channel": snippet.get("channelTitle").and_then(Value::as_str).unwrap_or("Unknown channel"),
        "channelId": snippet.get("channelId").and_then(Value::as_str).unwrap_or(""),
        "description": snippet.get("description").and_then(Value::as_str).unwrap_or(""),
        "publishedAt": snippet.get("publishedAt").cloned().unwrap_or(Value::Null),
        "thumbnail": thumbnail(item),
        "views": views,
        "likes": likes,
        "duration": details.get("duration").cloned().unwrap_or(Value::Null)
    })
}

const AD_FIELDS: &[&str] = &[
    "ad",
    "isAd",
    "ads",
    "adSlots",
    "adTag",
    "promoted",
    "promotion",
    "sponsored",
    "coupon",
    "engagementPanels",
    "storyboards",
    "storyboardSpec",
    "playerStoryboardSpecRenderer",
    "trackingUrl",
    "trackingUrls",
    "merchandise",
    "product",
];

/// Drop promotional items/fields before they can reach the browser.
pub fn clean_items(items: &[Value], adblock: bool) -> Vec<Value> {
    items
        .iter()
        .filter(|item| {
            let flagged = ["ad", "isAd", "promoted", "sponsored", "coupon"]
                .iter()
                .any(|key| item.get(*key).and_then(Value::as_bool) == Some(true));
            let stuffed = ["ads", "adSlots", "engagementPanels"]
                .iter()
                .any(|key| item.get(*key).is_some_and(|value| !value.is_null()));
            if flagged || stuffed {
                return false;
            }
            if !adblock {
                return true;
            }
            let channel = item
                .get("authorId")
                .or_else(|| item.get("channelId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let channel = if channel.is_empty() {
                item.get("snippet")
                    .and_then(|snippet| snippet.get("channelId"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            } else {
                channel
            };
            !crate::guard::is_blocked_channel(channel)
        })
        .filter_map(|item| {
            let cleaned = item.as_object()?;
            let mut copy = cleaned.clone();
            for key in AD_FIELDS {
                copy.remove(*key);
            }
            if copy.contains_key("videoId") {
                Some(map_video(&Value::Object(copy)))
            } else {
                Some(map_official_video(&Value::Object(copy)))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn strips_promotional_fields_and_blocked_channels() {
        let items = vec![
            json!({"videoId": "abcdefghijk", "title": "Real", "authorId": "UCreal00000000000000000", "storyboards": []}),
            json!({"videoId": "abcdefghij1", "title": "Blocked", "authorId": "UCadsample00000000000000"}),
            json!({"videoId": "abcdefghij2", "title": "Ad", "authorId": "UCreal00000000000000000", "promoted": true}),
            json!({"videoId": "abcdefghij3", "title": "Clean", "authorId": "UCreal00000000000000000"}),
        ];
        let cleaned = clean_items(&items, true);
        assert_eq!(cleaned.len(), 2);
        assert_eq!(cleaned[0]["title"], "Real");
        assert!(cleaned[0].get("storyboards").is_none());
    }

    #[test]
    fn keeps_blocked_channels_when_adblock_is_off() {
        let items = vec![
            json!({"videoId": "abcdefghijk", "title": "x", "authorId": "UCadsample00000000000000"}),
        ];
        assert_eq!(clean_items(&items, false).len(), 1);
    }

    #[test]
    fn prefers_api_provided_widescreen_thumbnail() {
        let item = json!({
            "videoId": "abcdefghijk",
            "videoThumbnails": [
                {"quality": "default", "url": "https://inv.example/vi/abcdefghijk/default.jpg", "width": 120, "height": 90},
                {"quality": "high", "url": "https://inv.example/vi/abcdefghijk/hqdefault.jpg", "width": 480, "height": 360},
                {"quality": "medium", "url": "//inv.example/vi/abcdefghijk/mqdefault.jpg", "width": 320, "height": 180},
                {"quality": "maxres", "url": "https://inv.example/vi/abcdefghijk/maxres.jpg", "width": 1280, "height": 720}
            ]
        });
        let url = thumbnail(&item);
        assert!(url.starts_with("/api/image?u="));
        assert!(url.contains("mqdefault.jpg"), "got {url}");
        assert!(url.contains("https%3A%2F%2Finv.example"), "got {url}");
    }

    #[test]
    fn reads_youtube_snippet_thumbnails_and_falls_back() {
        let item = json!({
            "id": "abcdefghijk",
            "snippet": {"thumbnails": {
                "default": {"url": "https://i.ytimg.com/vi/abcdefghijk/default.jpg", "width": 120, "height": 90},
                "medium": {"url": "https://i.ytimg.com/vi/abcdefghijk/mqdefault.jpg", "width": 320, "height": 180}
            }}
        });
        assert!(thumbnail(&item).contains("mqdefault.jpg"));
        assert_eq!(
            thumbnail(&json!({"videoId": "abcdefghijk"})),
            "/api/thumb/abcdefghijk/hqdefault.jpg"
        );
    }

    #[test]
    fn empty_map_is_a_noop() {
        let empty: Map<String, Value> = Map::new();
        assert!(clean_items(&[Value::Object(empty)], true).len() == 1);
    }
}

pub fn best_stream(value: &Value, instance: &str) -> Option<String> {
    fn resolve(url: &str, instance: &str) -> String {
        if url.starts_with('/') {
            format!("{instance}{url}")
        } else {
            url.to_string()
        }
    }

    fn best_video(formats: &[Value], instance: &str) -> Option<String> {
        let mut best: Option<(u32, String)> = None;
        for format in formats {
            let Some(url) = format.get("url").and_then(Value::as_str) else {
                continue;
            };
            let mime = format.get("type").and_then(Value::as_str).unwrap_or("");
            if !mime.starts_with("video/") {
                continue;
            }
            let label = format
                .get("resolution")
                .and_then(Value::as_str)
                .or_else(|| format.get("qualityLabel").and_then(Value::as_str))
                .unwrap_or("");
            let resolution = label
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0);
            if best
                .as_ref()
                .is_none_or(|(current, _)| resolution >= *current)
            {
                best = Some((resolution, resolve(url, instance)));
            }
        }
        best.map(|(_, url)| url)
    }

    let progressive = value
        .get("formatStreams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if let Some(url) = best_video(progressive, instance) {
        return Some(url);
    }

    for key in ["hlsUrl", "dashUrl"] {
        if let Some(url) = value.get(key).and_then(Value::as_str) {
            return Some(resolve(url, instance));
        }
    }

    let adaptive = value
        .get("adaptiveFormats")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    best_video(adaptive, instance)
}
