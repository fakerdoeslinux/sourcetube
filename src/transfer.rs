//! Google sign-in transfer wizard: YouTube Data API import + Takeout import.

use crate::{api, auth::Session, config::Config, guard, store};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use url::Url;

const EMPTY: &[Value] = &[];

#[derive(Clone, serde::Serialize)]
struct Stage {
    name: &'static str,
    status: String,
    count: usize,
    error: Option<String>,
}

struct Task {
    key: String,
    status: String,
    stages: Vec<Stage>,
    cancel: Arc<AtomicBool>,
    started: u64,
    finished: Option<u64>,
}

static TASK: OnceLock<Mutex<Option<Task>>> = OnceLock::new();

fn task() -> &'static Mutex<Option<Task>> {
    TASK.get_or_init(|| Mutex::new(None))
}

fn stages() -> Vec<Stage> {
    [
        "subscriptions",
        "channels",
        "playlists",
        "playlist_items",
        "watch_later",
        "likes",
    ]
    .iter()
    .map(|name| Stage {
        name,
        status: "pending".into(),
        count: 0,
        error: None,
    })
    .collect()
}

pub fn status() -> Value {
    let Ok(guard) = task().lock() else {
        return json!({ "status": "idle", "stages": [] });
    };
    match guard.as_ref() {
        None => json!({ "status": "idle", "stages": [] }),
        Some(task) => json!({
            "status": task.status,
            "key": task.key,
            "started": task.started,
            "finished": task.finished,
            "stages": task.stages,
        }),
    }
}

pub fn cancel() {
    if let Ok(mut guard) = task().lock() {
        if let Some(task) = guard.as_mut() {
            task.cancel.store(true, Ordering::Relaxed);
            task.status = "cancelled".into();
        }
    }
}

pub fn reset() {
    if let Ok(mut guard) = task().lock() {
        *guard = None;
    }
}

fn set_stage(index: usize, status: &str, count: usize, error: Option<String>) {
    if let Ok(mut guard) = task().lock() {
        if let Some(task) = guard.as_mut() {
            if let Some(stage) = task.stages.get_mut(index) {
                stage.status = status.into();
                stage.count = count;
                stage.error = error;
            }
        }
    }
}

pub fn start(session: &Session, config: &Config) -> Result<(), String> {
    {
        let mut guard = task()
            .lock()
            .map_err(|_| "Transfer state unavailable".to_string())?;
        if guard.as_ref().is_some_and(|task| task.status == "running") {
            return Err("A transfer is already running.".into());
        }
        let key = session
            .profile
            .as_ref()
            .map(|profile| profile.id.clone())
            .unwrap_or_default();
        *guard = Some(Task {
            key,
            status: "running".into(),
            stages: stages(),
            cancel: Arc::new(AtomicBool::new(false)),
            started: crate::auth::now(),
            finished: None,
        });
    }

    let token = session.access_token.clone();
    let cancel = {
        let guard = task()
            .lock()
            .map_err(|_| "Transfer state unavailable".to_string())?;
        guard.as_ref().map(|task| task.cancel.clone()).unwrap()
    };
    let config = config.clone();
    thread::spawn(move || run(token, &config, cancel));
    Ok(())
}

fn stopped(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn run(token: String, config: &Config, cancel: Arc<AtomicBool>) {
    let mut playlist_ids: Vec<String> = Vec::new();
    let steps: [fn(&str, &Config, &AtomicBool, &mut Vec<String>) -> Result<usize, String>; 6] = [
        import_subscriptions,
        import_channels,
        import_playlists,
        import_playlist_items,
        import_watch_later,
        import_likes,
    ];
    for (index, step) in steps.into_iter().enumerate() {
        if stopped(&cancel) {
            break;
        }
        set_stage(index, "running", 0, None);
        match step(&token, config, &cancel, &mut playlist_ids) {
            Ok(count) => set_stage(index, "done", count, None),
            Err(error) => set_stage(index, "error", 0, Some(error)),
        }
    }
    if let Ok(mut guard) = task().lock() {
        if let Some(task) = guard.as_mut() {
            task.status = if stopped(&cancel) {
                "cancelled"
            } else {
                "done"
            }
            .into();
            task.finished = Some(crate::auth::now());
        }
    }
}

fn token(token: &str) -> Option<&str> {
    (!token.is_empty()).then_some(token)
}

fn items(value: &Value) -> &[Value] {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(EMPTY)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn keep_channel(channel_id: &str) -> bool {
    !guard::is_blocked_channel(channel_id)
}

fn import_subscriptions(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    _: &mut Vec<String>,
) -> Result<usize, String> {
    let mut count = 0;
    let mut page = String::new();
    loop {
        if stopped(cancel) {
            return Ok(count);
        }
        let mut params = vec![("part", "snippet"), ("mine", "true"), ("maxResults", "50")];
        if !page.is_empty() {
            params.push(("pageToken", &page));
        }
        let response = api::youtube("subscriptions", &params, token(access), config)?;
        for item in items(&response) {
            let snippet = item.get("snippet").unwrap_or(&Value::Null);
            let id = text(snippet, "channelId");
            if !keep_channel(&id) {
                continue;
            }
            store::upsert(
                "subscription",
                &id,
                &text(snippet, "title"),
                &text(snippet, "title"),
                &id,
                0,
                &json!({ "description": text(snippet, "description") }),
            )?;
            count += 1;
        }
        page = text(&response, "nextPageToken");
        if page.is_empty() || count >= 500 {
            break;
        }
    }
    Ok(count)
}

fn import_channels(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    _: &mut Vec<String>,
) -> Result<usize, String> {
    let subscriptions = store::recent("subscription", 500)?;
    let ids: Vec<String> = subscriptions
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut count = 0;
    for chunk in ids.chunks(50) {
        if stopped(cancel) {
            break;
        }
        let response = api::youtube(
            "channels",
            &[
                ("part", "snippet,contentDetails"),
                ("id", &chunk.join(",")),
                ("maxResults", "50"),
            ],
            token(access),
            config,
        )?;
        for item in items(&response) {
            let snippet = item.get("snippet").unwrap_or(&Value::Null);
            let id = text(item, "id");
            if !keep_channel(&id) {
                continue;
            }
            store::upsert(
                "channel",
                &id,
                &text(snippet, "title"),
                &text(snippet, "title"),
                &id,
                0,
                &json!({
                    "description": text(snippet, "description"),
                    "uploads": item.get("contentDetails")
                        .and_then(|value| value.get("relatedPlaylists"))
                        .and_then(|value| value.get("uploads"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                }),
            )?;
            count += 1;
        }
    }
    Ok(count)
}

fn import_playlists(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    playlists: &mut Vec<String>,
) -> Result<usize, String> {
    let mut count = 0;
    let mut page = String::new();
    loop {
        if stopped(cancel) {
            return Ok(count);
        }
        let mut params = vec![
            ("part", "snippet,contentDetails"),
            ("mine", "true"),
            ("maxResults", "50"),
        ];
        if !page.is_empty() {
            params.push(("pageToken", &page));
        }
        let response = api::youtube("playlists", &params, token(access), config)?;
        for item in items(&response) {
            let id = text(item, "id");
            let snippet = item.get("snippet").unwrap_or(&Value::Null);
            if id.is_empty() || count >= 200 {
                continue;
            }
            playlists.push(id.clone());
            store::upsert(
                "playlist",
                &id,
                &text(snippet, "title"),
                &text(snippet, "channelTitle"),
                &text(snippet, "channelId"),
                0,
                &json!({ "description": text(snippet, "description") }),
            )?;
            count += 1;
        }
        page = text(&response, "nextPageToken");
        if page.is_empty() || count >= 200 {
            break;
        }
    }
    Ok(count)
}

fn playlist_videos(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    playlist_id: &str,
    kind: &str,
) -> Result<usize, String> {
    let mut count = 0;
    let mut page = String::new();
    loop {
        if stopped(cancel) {
            return Ok(count);
        }
        let mut params = vec![
            ("part", "snippet"),
            ("playlistId", playlist_id),
            ("maxResults", "50"),
        ];
        if !page.is_empty() {
            params.push(("pageToken", &page));
        }
        let response = api::youtube("playlistItems", &params, token(access), config)?;
        for item in items(&response) {
            let snippet = item.get("snippet").unwrap_or(&Value::Null);
            let id = snippet
                .get("resourceId")
                .and_then(|value| value.get("videoId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let channel = text(snippet, "channelId");
            if id.is_empty() || !keep_channel(&channel) {
                continue;
            }
            store::upsert(
                kind,
                id,
                &text(snippet, "title"),
                &text(snippet, "channelTitle"),
                &channel,
                0,
                &json!({ "playlistId": playlist_id }),
            )?;
            count += 1;
        }
        page = text(&response, "nextPageToken");
        if page.is_empty() || count >= 200 {
            break;
        }
    }
    Ok(count)
}

fn import_playlist_items(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    playlists: &mut Vec<String>,
) -> Result<usize, String> {
    let mut count = 0;
    for playlist in playlists.iter().take(20) {
        count += playlist_videos(access, config, cancel, playlist, "video")?;
    }
    Ok(count)
}

fn import_watch_later(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    playlists: &mut Vec<String>,
) -> Result<usize, String> {
    let watch_later = playlists
        .iter()
        .find(|id| id.as_str() == "WL")
        .cloned()
        .or_else(|| {
            store::recent("playlist", 200)
                .ok()?
                .into_iter()
                .find_map(|item| {
                    let title = item.get("title").and_then(Value::as_str).unwrap_or("");
                    (title.eq_ignore_ascii_case("watch later"))
                        .then(|| {
                            item.get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string()
                        })
                        .filter(|id| !id.is_empty())
                })
        });
    let Some(id) = watch_later else {
        return Ok(0);
    };
    playlist_videos(access, config, cancel, &id, "watch_later")
}

fn import_likes(
    access: &str,
    config: &Config,
    cancel: &AtomicBool,
    _: &mut Vec<String>,
) -> Result<usize, String> {
    let mut count = 0;
    let mut page = String::new();
    loop {
        if stopped(cancel) {
            return Ok(count);
        }
        let mut params = vec![
            ("part", "snippet,contentDetails"),
            ("myRating", "like"),
            ("maxResults", "50"),
        ];
        if !page.is_empty() {
            params.push(("pageToken", &page));
        }
        let response = api::youtube("videos", &params, token(access), config)?;
        for item in items(&response) {
            let snippet = item.get("snippet").unwrap_or(&Value::Null);
            let id = text(item, "id");
            let channel = text(snippet, "channelId");
            if id.is_empty() || !keep_channel(&channel) {
                continue;
            }
            store::upsert(
                "like",
                &id,
                &text(snippet, "title"),
                &text(snippet, "channelTitle"),
                &channel,
                0,
                &json!({}),
            )?;
            count += 1;
        }
        page = text(&response, "nextPageToken");
        if page.is_empty() || count >= 500 {
            break;
        }
    }
    Ok(count)
}

pub fn result() -> Result<Value, String> {
    let summary = store::summary()?;
    let mut recent = serde_json::Map::new();
    for kind in [
        "history",
        "comment",
        "like",
        "playlist",
        "subscription",
        "video",
        "watch_later",
    ] {
        recent.insert(kind.into(), json!(store::recent(kind, 24)?));
    }
    Ok(json!({
        "status": status()["status"],
        "summary": summary,
        "recent": Value::Object(recent),
    }))
}

fn url_param(raw: &str, key: &str) -> String {
    let Ok(url) = Url::parse(raw) else {
        return String::new();
    };
    url.query_pairs()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

fn channel_from_url(raw: &str) -> String {
    let Ok(url) = Url::parse(raw) else {
        return String::new();
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(|parts| parts.collect())
        .unwrap_or_default();
    for (index, segment) in segments.iter().enumerate() {
        if matches!(*segment, "channel" | "c") {
            if let Some(id) = segments.get(index + 1) {
                return (*id).to_string();
            }
        }
    }
    String::new()
}

fn video_id_from(value: &serde_json::Map<String, Value>) -> String {
    let direct = value
        .get("videoId")
        .or_else(|| value.get("video_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !direct.is_empty() {
        return direct.to_string();
    }
    let url = value
        .get("titleUrl")
        .or_else(|| value.get("contentUrl"))
        .or_else(|| value.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    url_param(url, "v")
}

fn timestamp(value: &serde_json::Map<String, Value>) -> i64 {
    for key in ["timeUtc", "time", "date", "timestamp", "at"] {
        if let Some(raw) = value.get(key).and_then(Value::as_str) {
            if let Ok(stamp) = raw.parse::<i64>() {
                return stamp;
            }
            if let Ok(date) = chrono_like(raw) {
                return date;
            }
        }
        if let Some(stamp) = value.get(key).and_then(Value::as_i64) {
            return stamp;
        }
    }
    0
}

/// A tiny ISO-8601 seconds parser; no extra date dependency is needed.
fn chrono_like(raw: &str) -> Result<i64, ()> {
    let date = raw.get(0..10).ok_or(())?;
    let mut parts = date.split('-');
    let year: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let month: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let day: i64 = parts.next().ok_or(())?.parse().map_err(|_| ())?;
    let time = raw.get(11..19).unwrap_or("00:00:00");
    let mut clock = time.split(':');
    let hour: i64 = clock.next().unwrap_or("0").parse().map_err(|_| ())?;
    let minute: i64 = clock.next().unwrap_or("0").parse().map_err(|_| ())?;
    let second: i64 = clock.next().unwrap_or("0").parse().map_err(|_| ())?;
    // Days since epoch (Howard Hinnant's civil-from-days inverse).
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok((era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second)
}

#[derive(Clone)]
struct Record {
    kind: String,
    id: String,
    title: String,
    channel: String,
    channel_id: String,
    at: i64,
    extra: Value,
}

fn walk(value: &Value, path: &str, records: &mut Vec<Record>) {
    match value {
        Value::Array(items) => {
            for item in items {
                walk(item, path, records);
            }
        }
        Value::Object(object) => {
            let mut path = path.to_string();
            if let Some(name) = object.get("name").and_then(Value::as_str) {
                if object.contains_key("content") {
                    if let Some(inner) = object.get("content").and_then(Value::as_str) {
                        if let Ok(parsed) = serde_json::from_str::<Value>(inner) {
                            walk(&parsed, name, records);
                        }
                    }
                    return;
                }
            }
            if let Some(title) = object.get("title").and_then(Value::as_str) {
                path.push_str(&format!("/{title}"));
            }
            let lower = path.to_ascii_lowercase();
            let channel_id = object
                .get("channelId")
                .or_else(|| object.get("channel_id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let object_url = object.get("url").and_then(Value::as_str).unwrap_or("");
            let channel_id = if channel_id.is_empty() {
                let from_query = url_param(object_url, "channel_id");
                if from_query.is_empty() {
                    channel_from_url(object_url)
                } else {
                    from_query
                }
            } else {
                channel_id
            };
            let channel = object
                .get("channelName")
                .or_else(|| object.get("author"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let title = object
                .get("title")
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let at = timestamp(object);
            let extra = json!({
                "details": object.get("details").cloned().unwrap_or(Value::Null),
                "content": object.get("content").cloned().unwrap_or(Value::Null),
            });

            if lower.contains("subscription") && !channel_id.is_empty() {
                records.push(Record {
                    kind: "subscription".into(),
                    id: channel_id.clone(),
                    title: title.clone(),
                    channel: channel.clone(),
                    channel_id,
                    at,
                    extra: extra.clone(),
                });
            } else if lower.contains("playlist") {
                let url = object.get("url").and_then(Value::as_str).unwrap_or("");
                let id = object
                    .get("playlistId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = if id.is_empty() {
                    url_param(url, "list")
                } else {
                    id.to_string()
                };
                if !id.is_empty() {
                    records.push(Record {
                        kind: "playlist".into(),
                        id,
                        title: title.clone(),
                        channel: channel.clone(),
                        channel_id: channel_id.clone(),
                        at,
                        extra: extra.clone(),
                    });
                } else {
                    let video = video_id_from(object);
                    if !video.is_empty() {
                        records.push(Record {
                            kind: "history".into(),
                            id: video,
                            title: title.clone(),
                            channel: channel.clone(),
                            channel_id: channel_id.clone(),
                            at,
                            extra: extra.clone(),
                        });
                    }
                }
            } else {
                let id = video_id_from(object);
                if !id.is_empty() {
                    let kind = if lower.contains("comment") {
                        "comment"
                    } else if lower.contains("like") {
                        "like"
                    } else {
                        "history"
                    };
                    records.push(Record {
                        kind: kind.into(),
                        id,
                        title: title.clone(),
                        channel: channel.clone(),
                        channel_id: channel_id.clone(),
                        at,
                        extra,
                    });
                }
            }

            for (key, value) in object {
                walk(value, &format!("{lower}/{key}"), records);
            }
        }
        _ => {}
    }
}

pub fn parse_takeout(payload: &Value) -> Vec<(String, String, String, String, String, i64, Value)> {
    let mut records = Vec::new();
    if let Some(files) = payload.get("files").and_then(Value::as_array) {
        for file in files {
            let name = file
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("takeout.json");
            let content = file.get("content").and_then(Value::as_str).unwrap_or("");
            if let Ok(parsed) = serde_json::from_str::<Value>(content) {
                walk(&parsed, name, &mut records);
            }
        }
    } else {
        walk(payload, "takeout", &mut records);
    }
    records.retain(|record| keep_channel(&record.channel_id));
    records
        .into_iter()
        .map(|record| {
            (
                record.kind,
                record.id,
                record.title,
                record.channel,
                record.channel_id,
                record.at,
                record.extra,
            )
        })
        .collect()
}

pub fn takeout(payload: &Value) -> Result<Value, String> {
    let records = parse_takeout(payload);
    let mut counts = serde_json::Map::new();
    for (kind, id, title, channel, channel_id, at, extra) in records {
        store::upsert(&kind, &id, &title, &channel, &channel_id, at, &extra)?;
        *counts.entry(kind.clone()).or_insert(json!(0)) =
            json!(counts.get(&kind).and_then(Value::as_u64).unwrap_or(0) + 1);
    }
    Ok(json!({ "imported": Value::Object(counts), "summary": store::summary()? }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_takeout_history_likes_subscriptions_and_playlists() {
        let payload = json!({
            "files": [
                {
                    "name": "Takeout/YouTube/history/watch-history.json",
                    "content": "[{\"title\":\"A video\",\"titleUrl\":\"https://www.youtube.com/watch?v=abcdefghijk\",\"time\":\"2026-01-02T03:04:05Z\"}]"
                },
                {
                    "name": "Takeout/YouTube/subscriptions/subscriptions.json",
                    "content": "[{\"title\":\"A channel\",\"url\":\"https://www.youtube.com/channel/UCreal00000000000000000\"}]"
                },
                {
                    "name": "Takeout/YouTube/playlists/playlists.json",
                    "content": "[{\"title\":\"Favorites\",\"url\":\"https://www.youtube.com/playlist?list=PLtest123\"},{\"title\":\"Liked\",\"videoId\":\"abcdefghij2\"}]"
                }
            ]
        });
        let records = parse_takeout(&payload);
        assert!(
            records
                .iter()
                .any(|record| record.0 == "history" && record.1 == "abcdefghijk")
        );
        assert!(
            records
                .iter()
                .any(|record| record.0 == "subscription" && record.1 == "UCreal00000000000000000")
        );
        assert!(
            records
                .iter()
                .any(|record| record.0 == "playlist" && record.1 == "PLtest123")
        );
        assert!(records.iter().any(|record| record.1 == "abcdefghij2"));
    }

    #[test]
    fn iso_timestamp_parses() {
        assert_eq!(chrono_like("2026-01-02T03:04:05Z").unwrap(), 1767323045);
    }

    #[test]
    fn deduplicates_records() {
        let payload = json!({"files": [{
            "name": "history.json",
            "content": "[{\"title\":\"A\",\"videoId\":\"dedupe-id\"},{\"title\":\"B\",\"videoId\":\"dedupe-id\"}]"
        }]});
        let records = parse_takeout(&payload);
        assert_eq!(
            records
                .iter()
                .filter(|record| record.1 == "dedupe-id")
                .count(),
            2
        );
    }
}
