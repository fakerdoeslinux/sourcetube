use rand::{RngCore, rng};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub app_origin: String,
    pub redirect_uri: String,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub youtube_api_key: String,
    pub session_secret: [u8; 32],
    pub invidious_instances: Vec<String>,
    /// Companion media hosts allowed by the DASH segment proxy, in addition
    /// to configured Invidious hosts and *.googlevideo.com.
    pub allowed_hosts: Vec<String>,
    pub public_dir: PathBuf,
    pub strict_port: bool,
    pub port_in_use_fallback: bool,
    pub secure_cookies: bool,
}

impl Config {
    pub fn load() -> Self {
        load_env_file();

        let requested_port = env::var("PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8787);
        let mut app_origin =
            env::var("APP_ORIGIN").unwrap_or_else(|_| format!("http://localhost:{requested_port}"));
        if !app_origin.starts_with("http://") && !app_origin.starts_with("https://") {
            app_origin = format!("http://{app_origin}");
        }
        let secure_cookies = app_origin.starts_with("https://");
        let redirect_uri = env::var("GOOGLE_REDIRECT_URI")
            .unwrap_or_else(|_| format!("{app_origin}/api/auth/callback"));

        let session_secret = match env::var("SESSION_SECRET") {
            Ok(value) if !value.is_empty() && value.len() >= 32 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&value.as_bytes()[..32]);
                key
            }
            Ok(value) if !value.is_empty() => {
                let mut key = [0u8; 32];
                let bytes = value.as_bytes();
                for index in 0..32 {
                    key[index] = bytes[index % bytes.len()];
                }
                key
            }
            _ => random_key(),
        };

        let invidious_instances = env::var("INVIDIOUS_INSTANCE")
            .unwrap_or_else(|_| "https://yt.chocolatemoo53.com".to_string())
            .split(',')
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .collect();

        Self {
            port: requested_port,
            app_origin,
            redirect_uri,
            google_client_id: env::var("GOOGLE_CLIENT_ID").unwrap_or_default(),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default(),
            youtube_api_key: env::var("YOUTUBE_API_KEY").unwrap_or_default(),
            session_secret,
            invidious_instances,
            allowed_hosts: env::var("SOURCETUBE_ALLOWED_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
                .filter(|value| !value.is_empty())
                .collect(),
            public_dir: discover_public_dir(),
            strict_port: env::var("STRICT_PORT")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
            port_in_use_fallback: false,
            secure_cookies,
        }
    }

    pub fn data_dir() -> PathBuf {
        env::var_os("SOURCETUBE_DATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("XDG_DATA_HOME").map(|value| PathBuf::from(value).join("sourcetube"))
            })
            .or_else(|| {
                env::var_os("HOME")
                    .map(|value| PathBuf::from(value).join(".local/share/sourcetube"))
            })
            .unwrap_or_else(|| PathBuf::from("sourcetube-data"))
    }

    pub fn runtime_dir() -> PathBuf {
        env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("TMPDIR").map(PathBuf::from))
            .or_else(|| env::var_os("TEMP").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    pub fn ensure_public_dir(&self) {
        if !self.public_dir.join("index.html").is_file() {
            eprintln!(
                "SourceTube static assets not found at {}",
                self.public_dir.display()
            );
            std::process::exit(1);
        }
    }
}

fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rng().fill_bytes(&mut key);
    key
}

fn load_env_file() {
    let mut candidates = Vec::new();
    if let Ok(home) = env::var("HOME") {
        candidates.push(PathBuf::from(home).join(".config/sourcetube/.env"));
    }
    candidates.push(PathBuf::from(".env"));
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join(".env"));
            candidates.push(parent.join("../.env"));
        }
    }

    for path in candidates {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if env::var(name).is_err() {
                unsafe { env::set_var(name, value) };
            }
        }
    }
}

fn discover_public_dir() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(value) = env::var("SOURCETUBE_PUBLIC") {
        candidates.push(PathBuf::from(value));
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("public"));
            candidates.push(parent.join("../share/sourcetube/public"));
        }
    }
    candidates.push(PathBuf::from("public"));
    candidates
        .into_iter()
        .find(|path| path.join("index.html").is_file())
        .unwrap_or_else(|| PathBuf::from("public"))
}
