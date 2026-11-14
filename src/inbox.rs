//! One library window owns the inbox. A second open hands it a torrent and exits.
//!
//! The lock is `flock` on a file under the engine directory. It is released when
//! the owning window closes. This does not ignore `SIGCHLD`; the engine child is
//! reaped on its own waiter thread.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::layout::Layout;

pub struct UiLock {
    file: File,
}

impl std::fmt::Debug for UiLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("UiLock")
    }
}

impl Drop for UiLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub enum Claim {
    Owner(UiLock),
    Busy,
}

pub fn try_claim(layout: &Layout) -> Result<Claim, String> {
    ensure_dir(&layout.engine_dir())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(layout.ui_lock_path())
        .map_err(|err| crate::access::plain(&format!("Could not open the library lock: {err}")))?;
    // Non-blocking exclusive lock. The kernel drops it if this process exits.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(Claim::Owner(UiLock { file }));
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN) {
        return Ok(Claim::Busy);
    }
    Err(crate::access::plain(&format!(
        "Could not lock the library: {err}"
    )))
}

pub fn append(layout: &Layout, sources: &[String]) -> Result<(), String> {
    with_inbox(layout, |file| {
        file.seek(SeekFrom::End(0))
            .map_err(|err| crate::access::plain(&format!("Could not write the inbox: {err}")))?;
        for source in sources {
            if source.is_empty() || source.contains(['\n', '\r', '\0']) {
                continue;
            }
            writeln!(file, "{source}").map_err(|err| {
                crate::access::plain(&format!("Could not write the inbox: {err}"))
            })?;
        }
        Ok(())
    })
}

pub fn take(layout: &Layout) -> Result<Vec<String>, String> {
    with_inbox(layout, |file| {
        file.seek(SeekFrom::Start(0))
            .map_err(|err| crate::access::plain(&format!("Could not read the inbox: {err}")))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .map_err(|err| crate::access::plain(&format!("Could not read the inbox: {err}")))?;
        file.set_len(0)
            .map_err(|err| crate::access::plain(&format!("Could not clear the inbox: {err}")))?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    })
}

fn with_inbox<T>(
    layout: &Layout,
    body: impl FnOnce(&mut File) -> Result<T, String>,
) -> Result<T, String> {
    ensure_dir(&layout.engine_dir())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(layout.inbox_path())
        .map_err(|err| crate::access::plain(&format!("Could not open the inbox: {err}")))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(crate::access::plain(&format!(
            "Could not lock the inbox: {}",
            std::io::Error::last_os_error()
        )));
    }
    let result = body(&mut file);
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
    result
}

fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|err| crate::access::io_sentence("create", path, &err))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|err| crate::access::io_sentence("protect", path, &err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn the_owner_receives_what_a_second_open_appends() {
        let n = AtomicU64::new(0).fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-inbox-{nanos}-{n}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::new(&home);

        let Claim::Owner(_lock) = try_claim(&layout).unwrap() else {
            panic!("the first window should own the inbox");
        };
        assert!(matches!(try_claim(&layout).unwrap(), Claim::Busy));

        append(
            &layout,
            &[
                "magnet:?xt=urn:btih:abc".into(),
                "skip\nme".into(),
                "hello.torrent".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            take(&layout).unwrap(),
            ["magnet:?xt=urn:btih:abc", "hello.torrent"]
        );
        assert!(take(&layout).unwrap().is_empty());

        drop(_lock);
        assert!(matches!(try_claim(&layout).unwrap(), Claim::Owner(_)));
        let _ = fs::remove_dir_all(home);
    }
}
