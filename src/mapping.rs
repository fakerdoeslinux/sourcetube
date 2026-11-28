use serde_json::{Value, json};

pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
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
        "thumbnail": format!("/api/thumb/{id}/hqdefault.jpg"),
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
        "thumbnail": format!("/api/thumb/{id}/hqdefault.jpg"),
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
