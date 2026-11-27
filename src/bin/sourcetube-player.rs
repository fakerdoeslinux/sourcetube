use libloading::{Library, Symbol};
use serde_json::Value;
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use ureq::tls::{TlsConfig, TlsProvider};

static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn agent() -> &'static ureq::Agent {
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

#[repr(C)]
struct MpvEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

type MpvCreate = unsafe extern "C" fn() -> *mut c_void;
type MpvSetOptionString = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int;
type MpvInitialize = unsafe extern "C" fn(*mut c_void) -> c_int;
type MpvCommand = unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int;
type MpvWaitEvent = unsafe extern "C" fn(*mut c_void, f64) -> *const MpvEvent;
type MpvTerminateDestroy = unsafe extern "C" fn(*mut c_void);

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("--resolve") {
        let id = args
            .get(2)
            .map(String::as_str)
            .unwrap_or_else(|| fail("usage: sourcetube-player --resolve <video-id>"));
        if !valid_id(id) {
            fail("invalid video ID");
        }
        let stream = resolve_stream(id).unwrap_or_else(|error| fail(&error));
        println!("{}", serde_json::to_string(&stream).unwrap_or_default());
        return;
    }
    let id = match args.get(1).map(String::as_str) {
        Some(id) if valid_id(id) => id.to_string(),
        Some("--url") => {
            if let Some(url) = args.get(2) {
                play(url, "SourceTube stream");
                return;
            }
            fail("usage: sourcetube-player <video-id> | --url <stream-url>");
        }
        _ => fail("usage: sourcetube-player <video-id> | --url <stream-url>"),
    };

    let stream = resolve_stream(&id).unwrap_or_else(|error| fail(&error));
    let url = stream.get("url").and_then(Value::as_str).unwrap_or("");
    let title = stream
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("SourceTube video");
    if url.is_empty() {
        fail("SourceTube returned an empty stream URL.");
    }
    play(url, title);
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'_' || value == b'-')
}

fn port() -> u16 {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let path = runtime.join("sourcetube-port");
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(port) = text.trim().parse::<u16>() {
            return port;
        }
    }
    env::var("PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8787)
}

fn resolve_stream(id: &str) -> Result<Value, String> {
    let url = format!("http://127.0.0.1:{}/api/stream?id={}", port(), id);
    let mut response = agent()
        .get(&url)
        .header("Accept", "application/json")
        .call()
        .map_err(|error| format!("SourceTube server is not reachable: {error}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Err(message.to_string());
    }
    Ok(value)
}

fn play(url: &str, title: &str) {
    if try_libmpv(url, title) {
        return;
    }
    let status = Command::new("mpv")
        .arg("--no-config")
        .arg("--force-window=yes")
        .arg("--cache=yes")
        .arg("--demuxer-max-bytes=32MiB")
        .arg("--demuxer-max-back-bytes=16MiB")
        .arg("--hwdec=auto-safe")
        .arg(format!("--title={title}"))
        .arg(url)
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => fail(&format!("mpv exited with status {status}")),
        Err(error) => fail(&format!("mpv is required for native playback: {error}")),
    }
}

fn try_libmpv(url: &str, title: &str) -> bool {
    let candidates = [
        "libmpv.so.2",
        "libmpv.so.1",
        "libmpv.so",
        "libmpv-2.dll",
        "libmpv.dll",
    ];
    let mut library = None;
    for name in candidates {
        if let Ok(loaded) = unsafe { Library::new(name) } {
            library = Some(loaded);
            break;
        }
    }
    let Some(library) = library else {
        return false;
    };

    unsafe {
        let create: Symbol<MpvCreate> = library.get(b"mpv_create\0").unwrap();
        let set_option: Symbol<MpvSetOptionString> =
            library.get(b"mpv_set_option_string\0").unwrap();
        let initialize: Symbol<MpvInitialize> = library.get(b"mpv_initialize\0").unwrap();
        let command: Symbol<MpvCommand> = library.get(b"mpv_command\0").unwrap();
        let wait_event: Symbol<MpvWaitEvent> = library.get(b"mpv_wait_event\0").unwrap();
        let terminate: Symbol<MpvTerminateDestroy> =
            library.get(b"mpv_terminate_destroy\0").unwrap();

        let context = create();
        if context.is_null() {
            return false;
        }
        for (name, value) in [
            ("no-config", "yes"),
            ("force-window", "yes"),
            ("keep-open", "yes"),
            ("cache", "yes"),
            ("demuxer-max-bytes", "32MiB"),
            ("demuxer-max-back-bytes", "16MiB"),
            ("hwdec", "auto-safe"),
            ("title", title),
        ] {
            let name = CString::new(name).unwrap();
            let value = CString::new(value).unwrap();
            let _ = set_option(context, name.as_ptr(), value.as_ptr());
        }
        if initialize(context) < 0 {
            terminate(context);
            return false;
        }

        let load = CString::new("loadfile").unwrap();
        let source = CString::new(url).unwrap();
        let replace = CString::new("replace").unwrap();
        let argv = [
            load.as_ptr(),
            source.as_ptr(),
            replace.as_ptr(),
            std::ptr::null(),
        ];
        if command(context, argv.as_ptr()) < 0 {
            eprintln!("libmpv failed to load the stream; falling back to mpv.");
            terminate(context);
            return false;
        }

        loop {
            let event = wait_event(context, 0.1);
            if event.is_null() {
                continue;
            }
            if (*event).event_id == 1 {
                break;
            }
        }
        terminate(context);
    }
    true
}
