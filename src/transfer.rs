use crate::api;
use crate::auth::Session;
use crate::config::Config;
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

const MAX_DATABASE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_IMPORT_ROWS: usize = 50_000;
static CANCELLED: AtomicBool = AtomicBool::new(false);
static STATUS: OnceLock<Mutex<TransferStatus>> = OnceLock::new();

#[derive(Clone, Serialize)]
pub struct TransferStage {
    pub name: String,
    pub status: String,
    pub detail: String,
}

#[derive(Clone, Serialize)]
pub struct TransferStatus {
    pub running: bool,
    pub stage: String,
    pub error: Option<String>,
    pub stages: Vec<TransferStage>,
}

impl Default for TransferStatus {
    fn default() -> Self {
        Self {
            running: false,
            stage: "idle".into(),
            error: None,
            stages: [
                "subscriptions",
                "channels",
                "playlists",
                "playlist items",
                "watch later",
                "liked videos",
            ]
            .into_iter()
            .map(|name| TransferStage {
                name: name.into(),
                status: "pending".into(),
                detail: String::new(),
            })
            .collect(),
        }
    }
}

fn status() -> &'static Mutex<TransferStatus> {
    STATUS.get_or_init(|| Mutex::new(TransferStatus::default()))
}

pub fn current_status() -> TransferStatus {
    status()
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default()
}

fn set_stage(name: &str, state: &str, detail: impl Into<String>) {
    if let Ok(mut status) = status().lock() {
        status.stage = name.to_string();
        if let Some(stage) = status.stages.iter_mut().find(|stage| stage.name == name) {
            stage.status = state.to_string();
            stage.detail = detail.into();
        }
    }
}

pub fn start(session: Session, config: Config) -> Result<(), String> {
    {
        let mut current = status().lock().map_err(|error| error.to_string())?;
        if current.running {
            return Err("A transfer is already running.".into());
        }
        *current = TransferStatus {
            running: true,
            ..TransferStatus::default()
        };
        CANCELLED.store(false, Ordering::SeqCst);
    }
    std::thread::Builder::new()
        .name("sourcetube-transfer".into())
        .spawn(move || {
            let result = import_api(&session, &config);
            if let Ok(mut current) = status().lock() {
                current.running = false;
                if CANCELLED.load(Ordering::SeqCst) {
                    current.stage = "cancelled".into();
                    for stage in &mut current.stages {
                        if stage.status == "running" {
                            stage.status = "cancelled".into();
                        }
                    }
                } else if let Err(error) = result {
                    current.stage = "failed".into();
                    current.error = Some(error);
                    for stage in &mut current.stages {
                        if stage.status == "running" {
                            stage.status = "failed".into();
                        }
                    }
                } else {
                    current.stage = "complete".into();
                }
            }
        })
        .map_err(|error| {
            if let Ok(mut current) = status().lock() {
                current.running = false;
                current.stage = "failed".into();
            }
            error.to_string()
        })?;
    Ok(())
}

pub fn cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
}

fn import_api(session: &Session, config: &Config) -> Result<(), String> {
    let mut db = open_database(&config.data_dir)?;
    let token = Some(session.access_token.as_str());

    check_cancelled()?;
    set_stage("subscriptions", "running", "Fetching subscribed channels.");
    let subscriptions = api::youtube(
        "subscriptions",
        &[
            ("part", "snippet,contentDetails"),
            ("mine", "true"),
            ("maxResults", "50"),
        ],
        token,
        config,
    )?;
    let subscription_items = array_items(&subscriptions);
    store_items(&mut db, "subscriptions", subscription_items)?;
    let channel_ids = subscription_items
        .iter()
        .filter_map(|item| {
            item.pointer("/snippet/resourceId/channelId")
                .and_then(Value::as_str)
        })
        .take(50)
        .collect::<Vec<_>>();
    set_stage(
        "subscriptions",
        "done",
        format!("Saved {} subscriptions.", subscription_items.len()),
    );

    check_cancelled()?;
    set_stage("channels", "running", "Fetching channel metadata.");
    if !channel_ids.is_empty() {
        let channels = api::youtube(
            "channels",
            &[
                ("part", "snippet,contentDetails"),
                ("id", &channel_ids.join(",")),
                ("maxResults", "50"),
            ],
            token,
            config,
        )?;
        store_items(&mut db, "channels", array_items(&channels))?;
    }
    set_stage(
        "channels",
        "done",
        format!("Saved {} channel records.", channel_ids.len()),
    );

    check_cancelled()?;
    set_stage("playlists", "running", "Fetching saved playlists.");
    let playlists_response = api::youtube(
        "playlists",
        &[
            ("part", "snippet,contentDetails"),
            ("mine", "true"),
            ("maxResults", "50"),
        ],
        token,
        config,
    )?;
    let playlists = array_items(&playlists_response);
    store_items(&mut db, "playlists", playlists)?;
    let playlist_ids = playlists
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .take(50)
        .collect::<Vec<_>>();
    set_stage(
        "playlists",
        "done",
        format!("Saved {} playlists.", playlists.len()),
    );

    check_cancelled()?;
    set_stage(
        "playlist items",
        "running",
        "Fetching items for saved playlists.",
    );
    let mut item_count = 0;
    for playlist_id in &playlist_ids {
        check_cancelled()?;
        let response = api::youtube(
            "playlistItems",
            &[
                ("part", "snippet,contentDetails"),
                ("playlistId", playlist_id),
                ("maxResults", "50"),
            ],
            token,
            config,
        )?;
        let items = array_items(&response);
        item_count += items.len();
        store_items(&mut db, "playlist_items", items)?;
    }
    set_stage(
        "playlist items",
        "done",
        format!("Saved {item_count} playlist items."),
    );

    check_cancelled()?;
    set_stage(
        "watch later",
        "running",
        "Checking the Watch Later playlist.",
    );
    match api::youtube(
        "playlistItems",
        &[
            ("part", "snippet,contentDetails"),
            ("playlistId", "WL"),
            ("maxResults", "50"),
        ],
        token,
        config,
    ) {
        Ok(items) => {
            let records = array_items(&items);
            store_items(&mut db, "watch_later", records)?;
            set_stage("watch later", "done", "Saved available Watch Later items.");
        }
        Err(_) => set_stage(
            "watch later",
            "done",
            "Watch Later is not available through this API.",
        ),
    }

    check_cancelled()?;
    set_stage(
        "liked videos",
        "running",
        "Fetching videos liked by this account.",
    );
    match api::youtube(
        "videos",
        &[
            ("part", "snippet,contentDetails"),
            ("myRating", "like"),
            ("maxResults", "50"),
        ],
        token,
        config,
    ) {
        Ok(likes) => {
            let records = array_items(&likes);
            store_items(&mut db, "likes", records)?;
            set_stage(
                "liked videos",
                "done",
                format!("Saved {} liked videos.", records.len()),
            );
        }
        Err(_) => set_stage(
            "liked videos",
            "done",
            "Liked videos are not available through this API.",
        ),
    }
    Ok(())
}

fn check_cancelled() -> Result<(), String> {
    if CANCELLED.load(Ordering::SeqCst) {
        Err("Transfer cancelled.".into())
    } else {
        Ok(())
    }
}

fn array_items(value: &Value) -> &[Value] {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn open_database(data_dir: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let path = data_dir.join("transfer.sqlite3");
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version > 1 {
        return Err(format!(
            "Unsupported transfer database schema version {version}."
        ));
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS imported_records (
                kind TEXT NOT NULL,
                record_id TEXT NOT NULL,
                payload TEXT NOT NULL,
                PRIMARY KEY (kind, record_id)
             );
             CREATE INDEX IF NOT EXISTS imported_records_kind ON imported_records(kind);
             PRAGMA user_version=1;",
        )
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "max_page_count", 65_536_i64)
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn store_items(db: &mut Connection, kind: &str, items: &[Value]) -> Result<(), String> {
    if items.len() > MAX_IMPORT_ROWS {
        return Err("Import contains too many records.".into());
    }
    let transaction = db.transaction().map_err(|error| error.to_string())?;
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT INTO imported_records(kind, record_id, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, record_id) DO UPDATE SET payload=excluded.payload",
            )
            .map_err(|error| error.to_string())?;
        for (index, item) in items.iter().enumerate() {
            let record_id = item
                .pointer("/contentDetails/videoId")
                .or_else(|| item.pointer("/snippet/resourceId/videoId"))
                .or_else(|| item.pointer("/snippet/resourceId/channelId"))
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{kind}-{index}-{}", item.to_string().len()));
            let payload = serde_json::to_string(item).map_err(|error| error.to_string())?;
            if payload.len() > 256 * 1024 {
                continue;
            }
            statement
                .execute(params![kind, record_id, payload])
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let db_size = db
        .path()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if db_size > MAX_DATABASE_BYTES {
        return Err("Transfer database exceeded its 256 MiB size limit.".into());
    }
    Ok(())
}

pub fn import_takeout(filename: &str, data: &Value, data_dir: &Path) -> Result<usize, String> {
    let mut db = open_database(data_dir)?;
    let kind = filename.to_ascii_lowercase();
    let kind = if kind.contains("watch") || kind.contains("history") {
        "watch_history"
    } else if kind.contains("like") {
        "likes"
    } else if kind.contains("playlist") {
        "playlists"
    } else if kind.contains("subscription") {
        "subscriptions"
    } else if kind.contains("comment") {
        "comments"
    } else {
        return Err("Unsupported Takeout JSON file. Use watch history, likes, playlists, subscriptions, or comments.".into());
    };
    let records = data
        .as_array()
        .ok_or_else(|| "Takeout JSON must contain a top-level array.".to_string())?;
    if records.len() > MAX_IMPORT_ROWS {
        return Err("Takeout file contains too many records.".into());
    }
    let rows = records
        .iter()
        .filter_map(|record| {
            let id = record
                .get("videoId")
                .or_else(|| record.get("channelId"))
                .or_else(|| record.get("playlistId"))
                .or_else(|| record.get("commentId"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    record
                        .get("titleUrl")
                        .and_then(Value::as_str)
                        .and_then(video_id_from_url)
                })
                .unwrap_or_else(|| stable_record_id(record));
            Some((id, record))
        })
        .collect::<Vec<_>>();
    let transaction = db.transaction().map_err(|error| error.to_string())?;
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT INTO imported_records(kind, record_id, payload) VALUES (?1, ?2, ?3)
                 ON CONFLICT(kind, record_id) DO UPDATE SET payload=excluded.payload",
            )
            .map_err(|error| error.to_string())?;
        for (id, record) in &rows {
            let payload = serde_json::to_string(record).map_err(|error| error.to_string())?;
            if payload.len() <= 256 * 1024 {
                statement
                    .execute(params![kind, id, payload])
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(rows.len())
}

fn video_id_from_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "v")
        .map(|(_, value)| value.into_owned())
}

fn stable_record_id(record: &Value) -> String {
    let digest = Sha256::digest(record.to_string().as_bytes());
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, digest)
}

pub fn results(data_dir: &Path) -> Result<Value, String> {
    let db = open_database(data_dir)?;
    let mut statement = db
        .prepare("SELECT kind, COUNT(*) FROM imported_records GROUP BY kind ORDER BY kind")
        .map_err(|error| error.to_string())?;
    let counts = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut records = serde_json::Map::new();
    for (kind, _) in &counts {
        let mut query = db
            .prepare(
                "SELECT payload FROM imported_records WHERE kind=?1 ORDER BY rowid DESC LIMIT 50",
            )
            .map_err(|error| error.to_string())?;
        let items = query
            .query_map([kind], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .filter_map(|item| serde_json::from_str::<Value>(&item).ok())
            .collect::<Vec<_>>();
        records.insert(kind.clone(), json!(items));
    }
    Ok(json!({
        "counts": counts.into_iter().collect::<std::collections::BTreeMap<_, _>>(),
        "records": records
    }))
}

#[cfg(test)]
mod tests {
    use super::{import_takeout, results};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn takeout_history_is_deduplicated_in_local_sqlite() {
        let directory = PathBuf::from(std::env::temp_dir())
            .join(format!("sourcetube-transfer-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let data = json!([{
            "title": "Watched an example",
            "titleUrl": "https://www.youtube.com/watch?v=abcdefghijk"
        }]);
        assert_eq!(
            import_takeout("watch-history.json", &data, &directory).unwrap(),
            1
        );
        assert_eq!(
            import_takeout("watch-history.json", &data, &directory).unwrap(),
            1
        );
        let stored = results(&directory).unwrap();
        assert_eq!(stored["counts"]["watch_history"], 1);
        std::fs::remove_file(directory.join("transfer.sqlite3")).unwrap();
        std::fs::remove_file(directory.join("transfer.sqlite3-shm")).ok();
        std::fs::remove_file(directory.join("transfer.sqlite3-wal")).ok();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn takeout_rejects_unrecognized_files() {
        let directory = PathBuf::from(std::env::temp_dir()).join(format!(
            "sourcetube-transfer-invalid-{}",
            std::process::id()
        ));
        assert!(import_takeout("unrelated.json", &json!([]), &directory).is_err());
        std::fs::remove_file(directory.join("transfer.sqlite3")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
