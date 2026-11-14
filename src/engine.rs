//! Download process. The library talks to it through [`crate::wire`].
//!
//! librqbit 9 fetches public torrents. It does not speak MSE or BitTorrent v2.
//! DHT state and the session file stay under Keel's data directory.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

use librqbit::api::TorrentIdOrHash;
use librqbit::dht::DhtPersistenceConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, DhtSessionConfig, ListenerOptions, ManagedTorrent, Session,
    SessionOptions, SessionPersistenceConfig, TorrentStatsState,
};

use crate::access;
use crate::geo;
use crate::layout::Layout;
use crate::source;
use crate::speed;
use crate::wire::{self, CreatedTorrent, PeerRow, Reply, Request, TorrentRow, TrackerRow};

const REPLY_LIMIT: usize = 2_000_000;

pub fn serve(layout: &Layout, dht: bool) -> Result<(), String> {
    if StdUnixStream::connect(layout.socket_path()).is_ok() {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|err| {
            access::explain(
                &layout.engine_dir(),
                &format!("Could not start the download engine: {err}"),
            )
        })?;
    runtime.block_on(run(layout, dht))
}

async fn run(layout: &Layout, dht: bool) -> Result<(), String> {
    fs::create_dir_all(layout.engine_dir())
        .map_err(|err| access::io_sentence("create", &layout.engine_dir(), &err))?;
    fs::set_permissions(layout.engine_dir(), fs::Permissions::from_mode(0o700))
        .map_err(|err| access::io_sentence("protect", &layout.engine_dir(), &err))?;
    fs::create_dir_all(layout.session_dir())
        .map_err(|err| access::io_sentence("create", &layout.session_dir(), &err))?;
    fs::set_permissions(layout.session_dir(), fs::Permissions::from_mode(0o700))
        .map_err(|err| access::io_sentence("protect", &layout.session_dir(), &err))?;

    let listener = match bind_socket(layout) {
        Ok(listener) => listener,
        Err(BindError::AlreadyRunning) => return Ok(()),
        Err(BindError::Failed(message)) => return Err(message),
    };

    let session = match open_session(layout, dht).await {
        Ok(session) => session,
        Err(err) => {
            drop(listener);
            let _ = fs::remove_file(layout.socket_path());
            return Err(err);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(EngineState::default());
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let accepted = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        match accepted {
            Ok(Ok((stream, _))) => {
                let session = Arc::clone(&session);
                let stop = Arc::clone(&stop);
                let layout = layout.clone();
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(err) = handle_client(stream, &session, &stop, &layout, &state).await
                    {
                        eprintln!("keel: {}", access::plain(&err));
                    }
                });
            }
            Ok(Err(err)) => {
                session.stop().await;
                let _ = fs::remove_file(layout.socket_path());
                return Err(access::explain(
                    &layout.socket_path(),
                    &format!("The engine connection failed: {err}"),
                ));
            }
            Err(_) => {}
        }
    }

    session.stop().await;
    let _ = fs::remove_file(layout.socket_path());
    Ok(())
}

enum BindError {
    AlreadyRunning,
    Failed(String),
}

fn bind_socket(layout: &Layout) -> Result<UnixListener, BindError> {
    let path = layout.socket_path();
    if StdUnixStream::connect(&path).is_ok() {
        return Err(BindError::AlreadyRunning);
    }
    // A previous engine can leave the socket file behind after it exits.
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|err| {
        if StdUnixStream::connect(&path).is_ok() {
            BindError::AlreadyRunning
        } else {
            BindError::Failed(access::io_sentence("listen on", &path, &err))
        }
    })?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    Ok(listener)
}

async fn open_session(layout: &Layout, dht: bool) -> Result<Arc<Session>, String> {
    match start_session(layout, dht, false).await {
        Ok(session) => Ok(session),
        Err(err) if dht => {
            eprintln!(
                "keel: retrying the listener on IPv4 after: {}",
                access::plain(&err)
            );
            start_session(layout, dht, true).await
        }
        Err(err) => Err(err),
    }
}

async fn start_session(
    layout: &Layout,
    dht: bool,
    ipv4_only: bool,
) -> Result<Arc<Session>, String> {
    let output = source::default_download_dir(layout);
    let _ = fs::create_dir_all(&output);
    // Keep the routing table next to the socket. The default path is an
    // unrelated folder in the user's config directory.
    let dht_config = if dht {
        Some(DhtSessionConfig {
            bootstrap_addrs: None,
            port: None,
            persistence: Some(DhtPersistenceConfig {
                dump_interval: None,
                config_filename: Some(layout.engine_dir().join("dht.json")),
            }),
        })
    } else {
        None
    };
    let listen = dht.then(|| ListenerOptions {
        ipv4_only,
        listen_addr: if ipv4_only {
            SocketAddr::from(([0, 0, 0, 0], 0))
        } else {
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
        },
        ..Default::default()
    });
    let opts = SessionOptions {
        fastresume: true,
        persistence: Some(SessionPersistenceConfig::Json {
            folder: Some(layout.session_dir()),
        }),
        client_name_and_version: Some(format!("Keel {}", env!("CARGO_PKG_VERSION"))),
        ipv4_only,
        dht: dht_config,
        disable_trackers: !dht,
        disable_local_service_discovery: !dht,
        listen,
        ..Default::default()
    };

    Session::new_with_opts(output, opts).await.map_err(|err| {
        access::explain(
            &layout.session_dir(),
            &format!("Could not open the download session: {err:#}"),
        )
    })
}

async fn handle_client(
    mut stream: tokio::net::UnixStream,
    session: &Arc<Session>,
    stop: &AtomicBool,
    layout: &Layout,
    state: &EngineState,
) -> Result<(), String> {
    let request = read_request(&mut stream).await?;
    let reply = dispatch(session, stop, layout, state, request).await;
    let mut bytes = serde_json::to_vec(&reply)
        .map_err(|err| access::plain(&format!("Could not write the reply: {err}")))?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .await
        .map_err(|err| access::plain(&format!("Could not write the reply: {err}")))?;
    Ok(())
}

async fn read_request(stream: &mut tokio::net::UnixStream) -> Result<Request, String> {
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|err| access::plain(&format!("Could not read the request: {err}")))?;
        if n == 0 {
            return Err("The library closed the connection.".into());
        }
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() >= REPLY_LIMIT {
            return Err("The request is too large.".into());
        }
        buf.push(byte[0]);
    }
    serde_json::from_slice(&buf)
        .map_err(|err| access::plain(&format!("Could not read the request: {err}")))
}

#[derive(Default)]
struct EngineState {
    rates: Mutex<HashMap<(u64, String), RateSample>>,
    hosts: Mutex<HashMap<String, String>>,
}

struct RateSample {
    fetched: u64,
    uploaded: u64,
    at_ms: u64,
}

async fn dispatch(
    session: &Arc<Session>,
    stop: &AtomicBool,
    layout: &Layout,
    state: &EngineState,
    request: Request,
) -> Reply {
    let result = match request {
        Request::List => Ok(Reply {
            torrents: list_torrents(session),
            ..ok_reply()
        }),
        Request::Add { source, folder } => add_torrent(session, layout, &source, &folder)
            .await
            .map(|()| ok_reply()),
        Request::Pause { id } => pause_torrent(session, id).await.map(|()| ok_reply()),
        Request::Resume { id } => resume_torrent(session, id).await.map(|()| ok_reply()),
        Request::Remove { id } => remove_torrent(session, id, false)
            .await
            .map(|()| ok_reply()),
        Request::Delete { id } => remove_torrent(session, id, true).await.map(|()| ok_reply()),
        Request::Details { id } => torrent_details(session, state, id).await,
        Request::Create {
            source,
            name,
            trackers,
            output_dir,
            seed,
        } => create_torrent_file(
            session,
            layout,
            &source,
            &name,
            &trackers,
            &output_dir,
            seed,
        )
        .await
        .map(|created| Reply {
            created: Some(created),
            ..ok_reply()
        }),
        Request::Shutdown => {
            stop.store(true, Ordering::SeqCst);
            Ok(ok_reply())
        }
    };
    match result {
        Ok(reply) => reply,
        Err(error) => Reply {
            ok: false,
            error,
            ..ok_reply()
        },
    }
}

fn ok_reply() -> Reply {
    Reply {
        ok: true,
        error: String::new(),
        torrents: Vec::new(),
        trackers: Vec::new(),
        peers: Vec::new(),
        created: None,
    }
}

fn list_torrents(session: &Session) -> Vec<TorrentRow> {
    let mut rows = session.with_torrents(|torrents| {
        let mut rows = Vec::new();
        for (id, handle) in torrents {
            let stats = handle.stats();
            let (down_bps, up_bps, peers) = match &stats.live {
                Some(live) => (
                    rate_bps(live.download_speed.mbps),
                    rate_bps(live.upload_speed.mbps),
                    live.snapshot.peer_stats.live,
                ),
                None => (0, 0, 0),
            };
            let hash = handle.info_hash().as_string();
            let name = handle
                .name()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("Torrent {}", &hash[..hash.len().min(8)]));
            let progress = if stats.total_bytes == 0 {
                0.0
            } else {
                (stats.progress_bytes as f64 / stats.total_bytes as f64).clamp(0.0, 1.0) as f32
            };
            rows.push(TorrentRow {
                id: id as u64,
                name,
                infohash: hash,
                state: state_name(&stats.state).to_owned(),
                progress,
                done: stats.progress_bytes,
                size: stats.total_bytes,
                down: wire::format_rate(down_bps as f64),
                up: wire::format_rate(up_bps as f64),
                down_bps,
                up_bps,
                peers,
                error: shown_error(
                    &handle.output_folder().display().to_string(),
                    &stats.error.unwrap_or_default(),
                ),
                folder: handle.output_folder().display().to_string(),
                finished: stats.finished,
            });
        }
        rows
    });
    rows.sort_by_key(|row| row.id);
    rows
}

fn rate_bps(mbps: f64) -> u64 {
    let bytes = mbps * 1024.0 * 1024.0;
    if bytes.is_finite() && bytes > 0.0 {
        bytes.round() as u64
    } else {
        0
    }
}

fn shown_error(folder: &str, error: &str) -> String {
    if error.is_empty() {
        String::new()
    } else {
        access::explain(Path::new(folder), error)
    }
}

fn state_name(state: &TorrentStatsState) -> &'static str {
    match state {
        TorrentStatsState::Initializing { .. } => "initializing",
        TorrentStatsState::Live => "live",
        TorrentStatsState::Paused => "paused",
        TorrentStatsState::Error => "error",
    }
}

async fn add_torrent(
    session: &Arc<Session>,
    layout: &Layout,
    source: &str,
    folder: &str,
) -> Result<(), String> {
    let source = source::normalize_one(source);
    if source.is_empty() {
        return Err("Choose a torrent file or a magnet link.".into());
    }
    let folder = source::prepare_download_dir(layout, folder)?;
    let add = if source.starts_with("magnet:") || is_info_hash(&source) {
        AddTorrent::from_url(source)
    } else {
        let bytes = fs::read(&source).map_err(|err| {
            access::explain(
                Path::new(&source),
                &format!("Could not read {source}: {err}"),
            )
        })?;
        AddTorrent::from_bytes(bytes)
    };
    // Resume writes over the files already on disk. librqbit refuses them otherwise.
    let opts = AddTorrentOptions {
        overwrite: true,
        output_folder: Some(folder.display().to_string()),
        ..Default::default()
    };
    session
        .add_torrent(add, Some(opts))
        .await
        .map_err(|err| access::explain(&folder, &format!("Could not add the torrent: {err:#}")))?;
    Ok(())
}

fn is_info_hash(source: &str) -> bool {
    source.len() == 40 && source.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn pause_torrent(session: &Session, id: u64) -> Result<(), String> {
    let handle = find(session, id)?;
    if handle.is_paused() {
        return Ok(());
    }
    session.pause(&handle).await.map_err(|err| {
        access::explain(
            handle.output_folder(),
            &format!("Could not pause the download: {err:#}"),
        )
    })
}

async fn resume_torrent(session: &Arc<Session>, id: u64) -> Result<(), String> {
    let handle = find(session, id)?;
    if !handle.is_paused() {
        return Ok(());
    }
    session.unpause(&handle).await.map_err(|err| {
        access::explain(
            handle.output_folder(),
            &format!("Could not resume the download: {err:#}"),
        )
    })
}

async fn remove_torrent(session: &Session, id: u64, delete_files: bool) -> Result<(), String> {
    let handle = find(session, id)?;
    // session.delete panics when magnet metadata has not arrived yet.
    if handle.metadata.load_full().is_none() {
        return Err(if delete_files {
            "Keel does not know which files belong to this torrent yet. Wait until its details arrive, then delete them.".into()
        } else {
            "This torrent is still fetching its details. Wait a moment, then remove it.".into()
        });
    }
    session
        .delete(TorrentIdOrHash::Id(handle.id()), delete_files)
        .await
        .map_err(|err| {
            let folder = handle.output_folder();
            if delete_files {
                access::explain(folder, &format!("Could not delete the files: {err:#}"))
            } else {
                access::explain(folder, &format!("Could not remove the download: {err:#}"))
            }
        })
}

async fn torrent_details(session: &Session, state: &EngineState, id: u64) -> Result<Reply, String> {
    let handle = find(session, id)?;
    let peers = peer_rows(state, id, &handle);
    let trackers = tracker_rows(state, &handle).await;
    Ok(Reply {
        peers,
        trackers,
        ..ok_reply()
    })
}

fn peer_rows(state: &EngineState, id: u64, handle: &ManagedTorrent) -> Vec<PeerRow> {
    let Some(live) = handle.live() else {
        return Vec::new();
    };
    let snapshot = with_default(|filter| live.per_peer_stats_snapshot(filter));
    let mut book = state.rates.lock().expect("peer speeds");
    let mut rows = Vec::new();
    for (addr, stats) in snapshot.peers {
        let (down_bps, up_bps, speeds_known) = sample_rate(
            &mut book,
            id,
            &addr,
            stats.counters.fetched_bytes,
            stats.counters.uploaded_bytes,
        );
        rows.push(PeerRow {
            country: geo::country_of_socket(&addr).unwrap_or_default(),
            addr,
            client: stats
                .client_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "—".to_owned()),
            down_bps,
            up_bps,
            speeds_known,
        });
    }
    if book.len() > 5000 {
        book.retain(|key, _| key.0 == id);
    }
    drop(book);
    rows.sort_by(|left, right| left.addr.cmp(&right.addr));
    rows.truncate(200);
    rows
}

fn with_default<T: Default, R>(apply: impl FnOnce(T) -> R) -> R {
    apply(T::default())
}

fn sample_rate(
    book: &mut HashMap<(u64, String), RateSample>,
    id: u64,
    addr: &str,
    fetched: u64,
    uploaded: u64,
) -> (u64, u64, bool) {
    let at_ms = now_ms();
    let key = (id, addr.to_owned());
    let measured = book.get(&key).and_then(|previous| {
        Some((
            speed::bytes_per_sec(previous.fetched, previous.at_ms, fetched, at_ms)?,
            speed::bytes_per_sec(previous.uploaded, previous.at_ms, uploaded, at_ms)?,
        ))
    });
    book.insert(
        key,
        RateSample {
            fetched,
            uploaded,
            at_ms,
        },
    );
    match measured {
        Some((down, up)) => (down, up, true),
        None => (0, 0, false),
    }
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

async fn tracker_rows(state: &EngineState, handle: &ManagedTorrent) -> Vec<TrackerRow> {
    let mut urls: Vec<_> = handle.shared.trackers.iter().cloned().collect();
    urls.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let mut rows = Vec::with_capacity(urls.len());
    let mut budget = 4u32;
    for url in urls {
        let host = url.host_str().unwrap_or("").to_owned();
        let country = country_for_host(state, &host, &mut budget).await;
        rows.push(TrackerRow {
            url: url.to_string(),
            host,
            country,
        });
    }
    rows
}

async fn country_for_host(state: &EngineState, host: &str, budget: &mut u32) -> String {
    if host.is_empty() {
        return String::new();
    }
    if let Some(cached) = state.hosts.lock().expect("tracker countries").get(host) {
        return cached.clone();
    }
    if *budget == 0 {
        return String::new();
    }
    *budget -= 1;
    let country = resolve_country(host).await;
    state
        .hosts
        .lock()
        .expect("tracker countries")
        .insert(host.to_owned(), country.clone());
    country
}

async fn resolve_country(host: &str) -> String {
    if let Some(code) = geo::country_of_socket(host) {
        return code;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return String::new();
    }
    let looked_up = tokio::time::timeout(
        Duration::from_millis(800),
        tokio::net::lookup_host((host, 80)),
    )
    .await;
    let Ok(Ok(addresses)) = looked_up else {
        return String::new();
    };
    for address in addresses {
        if let Some(code) = geo::country(address.ip()) {
            return code;
        }
    }
    String::new()
}

async fn create_torrent_file(
    session: &Arc<Session>,
    layout: &Layout,
    source: &str,
    name: &str,
    trackers: &[String],
    output_dir: &str,
    seed: bool,
) -> Result<CreatedTorrent, String> {
    let path = PathBuf::from(source.trim());
    if !path.is_absolute() {
        return Err("Choose a file or folder with its full path.".into());
    }
    ensure_shareable(&path)?;
    let disk_name = path
        .file_name()
        .and_then(|item| item.to_str())
        .filter(|item| !item.is_empty())
        .ok_or_else(|| "Choose a file or folder with a normal name.".to_owned())?
        .to_owned();
    let display = {
        let typed = name.trim();
        if typed.is_empty() {
            disk_name.clone()
        } else {
            typed.to_owned()
        }
    };
    let trackers: Vec<String> = trackers
        .iter()
        .map(|tracker| tracker.trim().to_owned())
        .filter(|tracker| !tracker.is_empty())
        .collect();
    let output = if output_dir.trim().is_empty() {
        path.parent()
            .filter(|parent| parent.is_dir())
            .ok_or_else(|| "That file has no folder to write the torrent into.".to_owned())?
            .to_path_buf()
    } else {
        source::prepare_download_dir(layout, output_dir)?
    };
    let torrent_path = output.join(format!("{}.torrent", torrent_file_stem(&display)));
    if torrent_path.exists() {
        return Err(format!("{} is already there.", torrent_path.display()));
    }
    // A single file is found by its real name. A custom title would hide it.
    let info_name = if path.is_file() || display == disk_name {
        None
    } else {
        Some(display.as_str())
    };
    let spawner = librqbit::spawn_utils::BlockingSpawner::new(1);
    let options = librqbit::CreateTorrentOptions {
        name: info_name,
        trackers: trackers.clone(),
        piece_length: None,
    };
    let created = librqbit::create_torrent(&path, options, &spawner)
        .await
        .map_err(|err| access::explain(&path, &format!("Could not create the torrent: {err:#}")))?;
    let bytes = created.as_bytes().map_err(|err| {
        access::explain(
            &output,
            &format!("Could not write the torrent file: {err:#}"),
        )
    })?;
    if torrent_path.exists() {
        return Err(format!("{} is already there.", torrent_path.display()));
    }
    fs::write(&torrent_path, &bytes)
        .map_err(|err| access::io_sentence("write", &torrent_path, &err))?;
    let infohash = created.info_hash().as_string();
    let piece_length = created.meta.info.data.piece_length;
    let magnet = source::magnet_uri(&infohash, &display, &trackers);
    if seed {
        let folder = created.output_folder.display().to_string();
        let hint = folder.clone();
        session
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    overwrite: true,
                    output_folder: Some(folder),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|err| {
                format!(
                    "The torrent file was written, but seeding did not start. {}",
                    access::explain(Path::new(&hint), &format!("{err:#}"))
                )
            })?;
    }
    Ok(CreatedTorrent {
        magnet,
        infohash,
        torrent_path: torrent_path.display().to_string(),
        piece_length,
    })
}

fn torrent_file_stem(name: &str) -> String {
    let mut stem = String::new();
    for ch in name.chars().take(120) {
        if ch.is_control() || matches!(ch, '/' | '\\' | '\0') {
            stem.push('_');
        } else {
            stem.push(ch);
        }
    }
    let stem = stem.trim().trim_matches('.').trim();
    if stem.is_empty() || stem == ".." {
        "shared".to_owned()
    } else {
        stem.to_owned()
    }
}

fn ensure_shareable(path: &Path) -> Result<(), String> {
    source::reject_links(path)?;
    let meta = fs::symlink_metadata(path).map_err(|err| access::io_sentence("read", path, &err))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a link. Choose the real file or folder.",
            path.display()
        ));
    }
    if path.to_str().is_none() {
        return Err(
            "That path is not valid text. Choose a file or folder with a normal name.".into(),
        );
    }
    if meta.is_dir() {
        walk_utf8(path)?;
        Ok(())
    } else if meta.is_file() {
        Ok(())
    } else {
        Err("Choose a file or a folder.".into())
    }
}

fn walk_utf8(dir: &Path) -> Result<(), String> {
    let read = fs::read_dir(dir).map_err(|err| access::io_sentence("read", dir, &err))?;
    for item in read {
        let item = item.map_err(|err| access::io_sentence("read", dir, &err))?;
        if item.file_name().to_str().is_none() {
            return Err(
                "A file in that folder has a name Keel cannot share. Rename it to plain text first."
                    .into(),
            );
        }
        let meta = match fs::symlink_metadata(item.path()) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk_utf8(&item.path())?;
        }
    }
    Ok(())
}

fn find(session: &Session, id: u64) -> Result<Arc<ManagedTorrent>, String> {
    let id = usize::try_from(id).map_err(|_| format!("Unknown download {id}."))?;
    session
        .get(TorrentIdOrHash::Id(id))
        .ok_or_else(|| "That download is no longer in the list.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn hello_torrent() -> Vec<u8> {
        let piece = [
            0xaa, 0xf4, 0xc6, 0x1d, 0xdc, 0xc5, 0xe8, 0xa2, 0xda, 0xbe, 0xde, 0x0f, 0x3b, 0x48,
            0x2c, 0xd9, 0xae, 0xa9, 0x43, 0x4d,
        ];
        let mut bytes = Vec::from(
            &b"d8:announce0:4:infod6:lengthi5e4:name5:hello12:piece lengthi16384e6:pieces20:"[..],
        );
        bytes.extend_from_slice(&piece);
        bytes.extend_from_slice(b"ee");
        bytes
    }

    #[test]
    fn adds_a_local_torrent_and_forgets_it_without_deleting_files() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-engine-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::new(&home);
        let download = home.join("out");
        fs::create_dir_all(&download).unwrap();
        let torrent = home.join("hello.torrent");
        fs::write(&torrent, hello_torrent()).unwrap();

        let engine_layout = layout.clone();
        let engine = thread::spawn(move || serve(&engine_layout, false));

        let started = Instant::now();
        loop {
            if engine.is_finished() {
                panic!("engine stopped: {:?}", engine.join().unwrap());
            }
            if wire::list(&layout).is_ok() {
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                let _ = wire::shutdown(&layout);
                panic!("engine did not open its socket: {:?}", engine.join());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let result = (|| {
            wire::call(
                &layout,
                &Request::Add {
                    source: torrent.display().to_string(),
                    folder: download.display().to_string(),
                },
                Duration::from_secs(20),
            )?;
            let rows = wire::list(&layout)?;
            if !rows.iter().any(|row| row.name == "hello") {
                return Err(format!("hello was not listed: {rows:?}"));
            }
            fs::write(download.join("keep.txt"), b"keep").map_err(|err| err.to_string())?;
            let id = rows
                .iter()
                .find(|row| row.name == "hello")
                .ok_or_else(|| format!("hello was not listed: {rows:?}"))?
                .id;
            let removed = wire::call(&layout, &Request::Remove { id }, Duration::from_secs(20))?;
            if !removed.ok {
                return Err(removed.error);
            }
            if fs::read(download.join("keep.txt")).ok().as_deref() != Some(b"keep".as_slice()) {
                return Err("keep.txt was deleted".into());
            }
            let rows = wire::list(&layout)?;
            if !rows.is_empty() {
                return Err(format!("torrent was still listed: {rows:?}"));
            }
            Ok(())
        })();

        let _ = wire::shutdown(&layout);
        let joined = engine.join().unwrap();
        let _ = fs::remove_dir_all(&home);
        result.unwrap();
        joined.unwrap();
    }

    #[test]
    fn creates_a_torrent_file_and_can_seed_it_without_the_network() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-share-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::new(&home);
        let source = home.join("notes.txt");
        fs::write(&source, b"hello").unwrap();
        let output = home.join("out");
        fs::create_dir_all(&output).unwrap();

        let engine_layout = layout.clone();
        let engine = thread::spawn(move || serve(&engine_layout, false));
        let started = Instant::now();
        loop {
            if engine.is_finished() {
                panic!("engine stopped: {:?}", engine.join().unwrap());
            }
            if wire::list(&layout).is_ok() {
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                let _ = wire::shutdown(&layout);
                panic!("engine did not open its socket");
            }
            thread::sleep(Duration::from_millis(50));
        }

        let result = (|| {
            let reply = wire::call(
                &layout,
                &Request::Create {
                    source: source.display().to_string(),
                    name: "notes.txt".into(),
                    trackers: Vec::new(),
                    output_dir: output.display().to_string(),
                    seed: false,
                },
                Duration::from_secs(30),
            )?;
            if !reply.ok {
                return Err(reply.error);
            }
            let created = reply.created.ok_or("missing torrent")?;
            if fs::read(&source).ok().as_deref() != Some(b"hello".as_slice()) {
                return Err("the source file changed".into());
            }
            if !Path::new(&created.torrent_path).is_file() {
                return Err(format!("torrent file missing: {}", created.torrent_path));
            }
            if !created.magnet.contains(&created.infohash) || created.infohash.len() != 40 {
                return Err(format!(
                    "magnet did not carry the info hash: {}",
                    created.magnet
                ));
            }
            let again = wire::call(
                &layout,
                &Request::Create {
                    source: source.display().to_string(),
                    name: "notes.txt".into(),
                    trackers: Vec::new(),
                    output_dir: output.display().to_string(),
                    seed: true,
                },
                Duration::from_secs(30),
            )?;
            if again.ok {
                return Err("a second torrent file was written over the first".into());
            }
            let seeded = home.join("clip.txt");
            fs::write(&seeded, b"clip").unwrap();
            let seeded_reply = wire::call(
                &layout,
                &Request::Create {
                    source: seeded.display().to_string(),
                    name: "clip.txt".into(),
                    trackers: Vec::new(),
                    output_dir: output.display().to_string(),
                    seed: true,
                },
                Duration::from_secs(30),
            )?;
            if !seeded_reply.ok {
                return Err(seeded_reply.error);
            }
            if fs::read(&seeded).ok().as_deref() != Some(b"clip".as_slice()) {
                return Err("seeding moved the source file".into());
            }
            let rows = wire::list(&layout)?;
            if !rows.iter().any(|row| row.name == "clip.txt") {
                return Err(format!("seeded torrent was not listed: {rows:?}"));
            }
            Ok(())
        })();

        let _ = wire::shutdown(&layout);
        let joined = engine.join().unwrap();
        let _ = fs::remove_dir_all(&home);
        result.unwrap();
        joined.unwrap();
    }
}
