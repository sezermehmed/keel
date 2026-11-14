//! Newline-delimited JSON between the library and the download process.
//!
//! One request per connection. The socket lives under the data directory.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::access;
use crate::inbox;
use crate::layout::Layout;

const REPLY_LIMIT: usize = 2_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    List,
    Add {
        source: String,
        folder: String,
    },
    Pause {
        id: u64,
    },
    Resume {
        id: u64,
    },
    Remove {
        id: u64,
    },
    Delete {
        id: u64,
    },
    Details {
        id: u64,
    },
    Create {
        source: String,
        name: String,
        trackers: Vec<String>,
        output_dir: String,
        seed: bool,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reply {
    pub ok: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub torrents: Vec<TorrentRow>,
    #[serde(default)]
    pub trackers: Vec<TrackerRow>,
    #[serde(default)]
    pub peers: Vec<PeerRow>,
    #[serde(default)]
    pub created: Option<CreatedTorrent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TrackerRow {
    pub url: String,
    pub host: String,
    pub country: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PeerRow {
    pub addr: String,
    pub client: String,
    pub down_bps: u64,
    pub up_bps: u64,
    pub speeds_known: bool,
    pub country: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreatedTorrent {
    pub magnet: String,
    pub infohash: String,
    pub torrent_path: String,
    pub piece_length: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TorrentRow {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub infohash: String,
    pub state: String,
    pub progress: f32,
    pub done: u64,
    pub size: u64,
    pub down: String,
    pub up: String,
    #[serde(default)]
    pub down_bps: u64,
    #[serde(default)]
    pub up_bps: u64,
    pub peers: u32,
    pub error: String,
    pub folder: String,
    pub finished: bool,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub torrents: Vec<TorrentRow>,
    pub error: String,
    pub inbox: Vec<String>,
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_rate(bytes_per_sec: f64) -> String {
    let bytes = if bytes_per_sec.is_finite() && bytes_per_sec > 0.0 {
        bytes_per_sec.round() as u64
    } else {
        0
    };
    format!("{}/s", format_bytes(bytes))
}

/// Ask the running engine for the list. `wait` paces the library poll.
/// A failed list keeps the previous rows in the window; this returns the error
/// and an empty list.
pub fn poll(layout: &Layout, binary: &Path, wait: bool, read_inbox: bool) -> Snapshot {
    if wait {
        thread::sleep(Duration::from_millis(800));
    }
    let inbox = if read_inbox {
        inbox::take(layout).unwrap_or_default()
    } else {
        Vec::new()
    };
    match ensure(layout, binary).and_then(|_| list(layout)) {
        Ok(torrents) => Snapshot {
            torrents,
            error: String::new(),
            inbox,
        },
        Err(error) => Snapshot {
            torrents: Vec::new(),
            error: access::explain(&layout.socket_path(), &error),
            inbox,
        },
    }
}

pub fn add(layout: &Layout, binary: &Path, source: &str, folder: &str) -> Result<(), String> {
    ensure(layout, binary)?;
    finish(call(
        layout,
        &Request::Add {
            source: source.to_owned(),
            folder: folder.to_owned(),
        },
        Duration::from_secs(20),
    )?)
}

pub fn pause(layout: &Layout, binary: &Path, id: u64) -> Result<(), String> {
    ensure(layout, binary)?;
    finish(call(
        layout,
        &Request::Pause { id },
        Duration::from_secs(20),
    )?)
}

pub fn resume(layout: &Layout, binary: &Path, id: u64) -> Result<(), String> {
    ensure(layout, binary)?;
    finish(call(
        layout,
        &Request::Resume { id },
        Duration::from_secs(20),
    )?)
}

pub fn remove(layout: &Layout, binary: &Path, id: u64) -> Result<(), String> {
    ensure(layout, binary)?;
    finish(call(
        layout,
        &Request::Remove { id },
        Duration::from_secs(20),
    )?)
}

pub fn delete_files(layout: &Layout, binary: &Path, id: u64) -> Result<(), String> {
    ensure(layout, binary)?;
    finish(call(
        layout,
        &Request::Delete { id },
        Duration::from_secs(20),
    )?)
}

pub fn details(layout: &Layout, binary: &Path, id: u64) -> Result<Reply, String> {
    ensure(layout, binary)?;
    let reply = call(layout, &Request::Details { id }, Duration::from_secs(8))?;
    if reply.ok {
        Ok(reply)
    } else {
        Err(blank(reply.error, "The peer list could not be read."))
    }
}

pub fn create(
    layout: &Layout,
    binary: &Path,
    source: &str,
    name: &str,
    trackers: &[String],
    output_dir: &str,
    seed: bool,
) -> Result<CreatedTorrent, String> {
    ensure(layout, binary)?;
    let reply = call(
        layout,
        &Request::Create {
            source: source.to_owned(),
            name: name.to_owned(),
            trackers: trackers.to_owned(),
            output_dir: output_dir.to_owned(),
            seed,
        },
        Duration::from_secs(120),
    )?;
    if reply.ok {
        reply
            .created
            .ok_or_else(|| "The torrent file was not returned.".to_owned())
    } else {
        Err(blank(reply.error, "Could not create the torrent."))
    }
}

pub fn list(layout: &Layout) -> Result<Vec<TorrentRow>, String> {
    let reply = call(layout, &Request::List, Duration::from_secs(8))?;
    if reply.ok {
        Ok(reply.torrents)
    } else {
        Err(blank(reply.error, "The download list could not be read."))
    }
}

/// `Ok(true)` when an engine accepted the request. `Ok(false)` when none is running.
pub fn shutdown(layout: &Layout) -> Result<bool, String> {
    match call(layout, &Request::Shutdown, Duration::from_secs(8)) {
        Ok(reply) if reply.ok => Ok(true),
        Ok(reply) => Err(blank(reply.error, "The download engine did not stop.")),
        Err(err) if err.contains("not running") => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn call(layout: &Layout, request: &Request, timeout: Duration) -> Result<Reply, String> {
    let mut stream = UnixStream::connect(layout.socket_path())
        .map_err(|_| "The download engine is not running.".to_owned())?;
    stream.set_read_timeout(Some(timeout)).map_err(talk)?;
    stream.set_write_timeout(Some(timeout)).map_err(talk)?;
    let mut bytes = serde_json::to_vec(request)
        .map_err(|err| access::plain(&format!("Could not write the request: {err}")))?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(talk)?;
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if buf.len() >= REPLY_LIMIT {
            return Err("The engine reply is too large.".into());
        }
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => buf.push(byte[0]),
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Err("The download engine took too long to answer.".into());
            }
            Err(err) => return Err(talk(err)),
        }
    }
    if buf.is_empty() {
        return Err("The download engine closed the connection.".into());
    }
    serde_json::from_slice(&buf)
        .map_err(|err| access::plain(&format!("Could not read the engine reply: {err}")))
}

fn finish(reply: Reply) -> Result<(), String> {
    if reply.ok {
        Ok(())
    } else {
        Err(blank(
            reply.error,
            "The download engine refused the request.",
        ))
    }
}

fn talk(err: impl std::fmt::Display) -> String {
    access::plain(&format!("Could not talk to the download engine: {err}"))
}

fn blank(error: String, fallback: &str) -> String {
    if error.is_empty() {
        fallback.to_owned()
    } else {
        error
    }
}

fn ensure(layout: &Layout, binary: &Path) -> Result<(), String> {
    if engine_up(layout) {
        // A library left open across an update still has the previous engine.
        // That process answers List, and it does not understand later requests.
        replace_stale_engine(layout, binary)?;
        if engine_up(layout) {
            return Ok(());
        }
    }
    static START: Mutex<()> = Mutex::new(());
    let _guard = START.lock().unwrap_or_else(|err| err.into_inner());
    if engine_up(layout) {
        return Ok(());
    }
    spawn_engine(layout, binary)?;
    for _ in 0..40 {
        if engine_up(layout) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "The download engine did not start. Details are in {}.",
        layout.engine_log_path().display()
    ))
}

fn engine_up(layout: &Layout) -> bool {
    call(layout, &Request::List, Duration::from_millis(400)).is_ok_and(|reply| reply.ok)
}

fn replace_stale_engine(layout: &Layout, binary: &Path) -> Result<(), String> {
    static CHECKED: AtomicBool = AtomicBool::new(false);
    if CHECKED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    if speaks_current_protocol(layout) {
        return Ok(());
    }
    let _ = shutdown(layout);
    for _ in 0..40 {
        if !engine_up(layout) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if engine_up(layout) {
        return Ok(());
    }
    spawn_engine(layout, binary)?;
    for _ in 0..40 {
        if engine_up(layout) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "The download engine did not start. Details are in {}.",
        layout.engine_log_path().display()
    ))
}

fn speaks_current_protocol(layout: &Layout) -> bool {
    // A missing torrent still gets a normal reply from a current engine.
    // An older engine closes the connection instead of answering.
    call(
        layout,
        &Request::Details { id: u64::MAX },
        Duration::from_millis(800),
    )
    .is_ok()
}

fn spawn_engine(layout: &Layout, binary: &Path) -> Result<(), String> {
    let log_path = layout.engine_log_path();
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).map_err(|err| access::io_sentence("create", parent, &err))?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|err| access::io_sentence("write", &log_path, &err))?;
    // Inherit the environment. Setting KEEL_HOME here would ignore a custom
    // XDG data directory and put the socket in a different folder than the window.
    // The waiter reaps the child. Dropping it would leave a zombie and would
    // not stop the download.
    let mut child = Command::new(binary)
        .arg("--engine")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|err| {
            access::explain(
                binary,
                &format!("Could not start the download engine: {err}"),
            )
        })?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_use_binary_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_rate(0.0), "0 B/s");
        assert_eq!(format_rate(1536.0), "1.5 KiB/s");
    }

    #[test]
    fn shutdown_without_an_engine_is_fine() {
        let home = std::env::temp_dir().join(format!(
            "keel-wire-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::new(&home);
        assert!(!shutdown(&layout).unwrap());
        assert!(!layout.socket_path().exists());
        let _ = fs::remove_dir_all(home);
    }
}
