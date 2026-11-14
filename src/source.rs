//! Names and save folders for torrents the library was asked to open.
//!
//! Magnet display names come from the `dn` value. `+` is a space only there.
//! A `file://` URI is turned into a filesystem path, where `+` stays `+`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::access;
use crate::install;
use crate::layout::Layout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub source: String,
    pub name: String,
    /// Lowercase 40-character hex infohash when the magnet has one.
    pub infohash: String,
}

/// Join a magnet that the desktop split on `?`, and turn `file://` URIs into paths.
pub fn normalize_args(args: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let current = &args[index];
        if current.ends_with("magnet:?")
            && args.get(index + 1).is_some_and(|next| {
                next.starts_with("xt=") || next.starts_with("dn=") || next.starts_with("tr=")
            })
        {
            out.push(format!("{current}{}", args[index + 1]));
            index += 2;
            continue;
        }
        out.push(normalize_one(current));
        index += 1;
    }
    out
}

pub fn describe(source: &str) -> Opened {
    let source = normalize_one(source);
    if let Some(name) = magnet_name(&source) {
        let infohash = magnet_infohash(&source);
        return Opened {
            source,
            name,
            infohash,
        };
    }
    if let Some(name) = torrent_display_name(Path::new(&source)) {
        return Opened {
            source,
            name,
            infohash: String::new(),
        };
    }
    let name = Path::new(&source)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Torrent")
        .to_owned();
    Opened {
        source,
        name,
        infohash: String::new(),
    }
}

/// `XDG_DOWNLOAD_DIR` when it expands to an absolute folder inside home.
/// Otherwise `~/Downloads`. The folder does not have to exist yet.
pub fn default_download_dir(layout: &Layout) -> PathBuf {
    let fallback = layout.home.join("Downloads");
    let Ok(text) = fs::read_to_string(layout.config_home.join("user-dirs.dirs")) else {
        return fallback;
    };
    for line in text.lines() {
        let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR") else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        if value.contains("..") || value.contains('\0') || value.contains('\n') {
            return fallback;
        }
        let home = layout.home.to_string_lossy();
        let expanded = value.replace("$HOME", home.as_ref());
        if expanded.contains('$') {
            return fallback;
        }
        let path = PathBuf::from(expanded);
        if path.is_absolute()
            && path.starts_with(&layout.home)
            && !path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return path;
        }
        return fallback;
    }
    fallback
}

/// Check a save folder. It may sit on another disk. It may not be Keel's
/// program folder, and it may not go through a symbolic link.
pub fn prepare_download_dir(layout: &Layout, input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Choose a folder to save the download.".into());
    }
    if trimmed.contains(['\0', '\n', '\r']) {
        return Err("The folder path cannot contain line breaks.".into());
    }
    let expanded = expand_tilde(&layout.home, trimmed);
    if !expanded.is_absolute() {
        return Err("Use a full path, or start with ~/.".into());
    }
    if expanded
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The folder path cannot contain '..'.".into());
    }
    reject_links(&expanded)?;
    if is_program_dir(layout, &expanded) {
        return Err("Choose a folder that is not where Keel keeps its own program.".into());
    }
    // An unplugged drive still has a parent on the system disk. Refuse before
    // create_dir, or Keel would plant an empty folder there.
    if let Some(problem) = access::inspect_live(&expanded) {
        return Err(problem.sentence());
    }

    let path = if expanded.exists() {
        let meta = fs::symlink_metadata(&expanded)
            .map_err(|err| access::io_sentence("inspect", &expanded, &err))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "{} is a link. Choose a real folder.",
                expanded.display()
            ));
        }
        if !meta.is_dir() {
            return Err("That path is a file, not a folder.".into());
        }
        expanded
            .canonicalize()
            .map_err(|err| access::io_sentence("open", &expanded, &err))?
    } else {
        let parent = expanded
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| "That folder cannot be reached.".to_owned())?;
        reject_links(parent)?;
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err("That folder's parent is not a real folder.".into());
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err("That folder's parent does not exist.".into());
            }
            Err(err) => {
                return Err(access::io_sentence("inspect", parent, &err));
            }
        }
        fs::create_dir(&expanded).map_err(|err| access::io_sentence("create", &expanded, &err))?;
        expanded
            .canonicalize()
            .map_err(|err| access::io_sentence("open", &expanded, &err))?
    };

    if is_program_dir(layout, &path) {
        return Err("Choose a folder that is not where Keel keeps its own program.".into());
    }
    Ok(path)
}

pub fn magnet_uri(infohash: &str, name: &str, trackers: &[String]) -> String {
    let mut magnet = format!("magnet:?xt=urn:btih:{}", infohash.to_ascii_lowercase());
    if !name.is_empty() {
        magnet.push_str("&dn=");
        magnet.push_str(&query_encode(name));
    }
    for tracker in trackers {
        if tracker.is_empty() {
            continue;
        }
        magnet.push_str("&tr=");
        magnet.push_str(&query_encode(tracker));
    }
    magnet
}

fn query_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn normalize_one(source: &str) -> String {
    file_uri_path(source).unwrap_or_else(|| source.to_owned())
}

fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file:")?.strip_prefix("//")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(path, false);
    if decoded.contains('\0') {
        return None;
    }
    Some(decoded)
}

fn magnet_infohash(source: &str) -> String {
    let Some(query) = source.strip_prefix("magnet:?") else {
        return String::new();
    };
    for part in query.split('&') {
        let Some(value) = part.strip_prefix("xt=") else {
            continue;
        };
        let Some(id) = value.rsplit(':').next() else {
            continue;
        };
        let id = percent_decode(id, false);
        if id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return id.to_ascii_lowercase();
        }
    }
    String::new()
}

fn magnet_name(source: &str) -> Option<String> {
    let query = source.strip_prefix("magnet:?")?;
    let mut display = None;
    let mut hash = None;
    for part in query.split('&') {
        if let Some(value) = part.strip_prefix("dn=") {
            let name = percent_decode(value, true);
            if !name.is_empty() {
                display = Some(name);
            }
        } else if let Some(value) = part.strip_prefix("xt=") {
            if let Some(id) = value.rsplit(':').next() {
                let id = percent_decode(id, false);
                if !id.is_empty() {
                    hash = Some(id);
                }
            }
        }
    }
    if let Some(name) = display {
        return Some(name);
    }
    let short = hash.unwrap_or_default();
    let short = if short.len() > 8 { &short[..8] } else { &short };
    if short.is_empty() {
        Some("Torrent".into())
    } else {
        Some(format!("Torrent {short}"))
    }
}

fn torrent_display_name(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 32 * 1024 * 1024 {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    bencode_info_name(&bytes)
}

fn percent_decode(input: &str, plus_is_space: bool) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if plus_is_space && bytes[index] == b'+' {
            out.push(b' ');
            index += 1;
        } else if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
            } else {
                out.push(bytes[index]);
                index += 1;
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn bencode_info_name(data: &[u8]) -> Option<String> {
    let mut reader = Reader { data, index: 0 };
    if reader.peek()? != b'd' {
        return None;
    }
    reader.index += 1;
    let mut name = None;
    while reader.peek()? != b'e' {
        let key = reader.string_bytes()?;
        if key == b"info" && name.is_none() {
            name = reader.dict_string(b"name");
        } else {
            reader.skip()?;
        }
    }
    name.filter(|name| !name.is_empty())
}

struct Reader<'a> {
    data: &'a [u8],
    index: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.index).copied()
    }

    fn dict_string(&mut self, want: &[u8]) -> Option<String> {
        if self.peek()? != b'd' {
            return None;
        }
        self.index += 1;
        let mut found = None;
        while self.peek()? != b'e' {
            let key = self.string_bytes()?;
            if key == want && found.is_none() {
                found = self.utf8_string();
            } else {
                self.skip()?;
            }
        }
        self.index += 1;
        found
    }

    fn utf8_string(&mut self) -> Option<String> {
        Some(String::from_utf8_lossy(&self.string_bytes()?).into_owned())
    }

    fn string_bytes(&mut self) -> Option<Vec<u8>> {
        let start = self.index;
        while self.peek()?.is_ascii_digit() {
            self.index += 1;
        }
        if self.index == start || self.peek()? != b':' {
            return None;
        }
        let length: usize = std::str::from_utf8(&self.data[start..self.index])
            .ok()?
            .parse()
            .ok()?;
        self.index += 1;
        let end = self.index.checked_add(length)?;
        if end > self.data.len() {
            return None;
        }
        let bytes = self.data[self.index..end].to_vec();
        self.index = end;
        Some(bytes)
    }

    fn skip(&mut self) -> Option<()> {
        match self.peek()? {
            b'd' => {
                self.index += 1;
                while self.peek()? != b'e' {
                    let _ = self.string_bytes()?;
                    self.skip()?;
                }
                self.index += 1;
                Some(())
            }
            b'l' => {
                self.index += 1;
                while self.peek()? != b'e' {
                    self.skip()?;
                }
                self.index += 1;
                Some(())
            }
            b'i' => {
                self.index += 1;
                while self.peek()? != b'e' {
                    self.index += 1;
                }
                self.index += 1;
                Some(())
            }
            b'0'..=b'9' => {
                let _ = self.string_bytes()?;
                Some(())
            }
            _ => None,
        }
    }
}

fn expand_tilde(home: &Path, input: &str) -> PathBuf {
    if input == "~" {
        home.to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(input)
    }
}

pub(crate) fn reject_links(folder: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in folder.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "{} is a link. Choose a real folder that does not go through a link.",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(access::io_sentence("inspect", &current, &err));
            }
        }
    }
    Ok(())
}

fn is_program_dir(layout: &Layout, path: &Path) -> bool {
    let mut reserved = vec![
        layout.default_install_dir(),
        layout.engine_dir(),
        layout.session_dir(),
    ];
    if let Some(installed) = install::locate(layout) {
        reserved.push(installed);
    }
    reserved
        .iter()
        .any(|dir| path == dir || path.starts_with(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-source-{nanos}-{n}"));
        fs::create_dir_all(&home).unwrap();
        home
    }

    const OPENED_MAGNET: &str = "magnet:?xt=urn:btih:4E101BF616AD6C7B0F9D5F86B92457A2CC0A9C4C&dn=I+Want+Your+Sex+%282026%29+%281080p+BluRay+x265+10bit+EAC3+Atmos+7.1+Celdra%29+%5BQxR%5D";

    #[test]
    fn decodes_the_magnet_display_name() {
        let opened = describe(OPENED_MAGNET);
        assert_eq!(
            opened.name,
            "I Want Your Sex (2026) (1080p BluRay x265 10bit EAC3 Atmos 7.1 Celdra) [QxR]"
        );
        assert!(!opened.name.contains("%282026%29"));
        assert!(!opened.name.contains('%'));
        assert_eq!(opened.infohash, "4e101bf616ad6c7b0f9d5f86b92457a2cc0a9c4c");
        assert_eq!(describe("magnet:?xt=urn:btih:abc").name, "Torrent abc");
        assert!(describe("magnet:?xt=urn:btih:abc").infohash.is_empty());
        assert_eq!(
            describe("magnet:?xt=urn:btih:4E101BF616AD6C7B0F9D5F86B92457A2CC0A9C4C").name,
            "Torrent 4E101BF6"
        );
    }

    #[test]
    fn joins_a_magnet_split_on_the_question_mark() {
        let args = normalize_args(vec![
            "magnet:?".into(),
            "xt=urn:btih:abc&dn=Hello+There".into(),
        ]);
        assert_eq!(args, ["magnet:?xt=urn:btih:abc&dn=Hello+There"]);
        assert_eq!(describe(&args[0]).name, "Hello There");
    }

    #[test]
    fn file_uri_keeps_plus_and_decodes_percent() {
        let opened = describe("file:///tmp/My%20Show+Cut.torrent");
        assert_eq!(opened.source, "/tmp/My Show+Cut.torrent");
        assert_eq!(opened.name, "My Show+Cut.torrent");
        let local = describe("file://localhost/tmp/a.torrent");
        assert_eq!(local.source, "/tmp/a.torrent");
    }

    #[test]
    fn reads_the_name_inside_a_torrent_file() {
        let home = scratch();
        let path = home.join("hello.torrent");
        let mut bytes = Vec::from(
            &b"d8:announce0:4:infod6:lengthi5e4:name5:hello12:piece lengthi16384e6:pieces20:"[..],
        );
        bytes.extend_from_slice(&[0u8; 20]);
        bytes.extend_from_slice(b"ee");
        fs::write(&path, &bytes).unwrap();
        assert_eq!(describe(&path.display().to_string()).name, "hello");
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn download_folder_rules() {
        let home = scratch();
        let layout = Layout::new(&home);
        fs::create_dir_all(home.join("Downloads")).unwrap();
        fs::create_dir_all(layout.config_home.join("user-dirs.dirs").parent().unwrap()).unwrap();
        fs::write(
            layout.config_home.join("user-dirs.dirs"),
            "XDG_DOWNLOAD_DIR=\"$HOME/Videos\"\n",
        )
        .unwrap();
        assert_eq!(default_download_dir(&layout), home.join("Videos"));

        let created = prepare_download_dir(&layout, "~/Downloads/movies").unwrap();
        assert_eq!(
            created,
            home.join("Downloads/movies").canonicalize().unwrap()
        );

        let outside = std::env::temp_dir().join(format!(
            "keel-outside-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            prepare_download_dir(&layout, &outside.display().to_string()).unwrap(),
            outside.canonicalize().unwrap()
        );

        assert!(prepare_download_dir(&layout, "Downloads").is_err());
        assert!(prepare_download_dir(&layout, "~/Downloads/../secret").is_err());
        assert!(prepare_download_dir(&layout, &format!("{}\0", home.display())).is_err());
        assert!(prepare_download_dir(&layout, "~/Downloads/missing/child").is_err());

        let install = layout.default_install_dir();
        fs::create_dir_all(&install).unwrap();
        assert!(prepare_download_dir(&layout, &install.display().to_string()).is_err());
        assert!(prepare_download_dir(&layout, &layout.engine_dir().display().to_string()).is_err());

        let real = home.join("real-save");
        fs::create_dir_all(&real).unwrap();
        let link = home.join("linked-save");
        symlink(&real, &link).unwrap();
        assert!(prepare_download_dir(&layout, &link.display().to_string()).is_err());

        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn an_unmounted_volume_is_refused_before_it_is_created() {
        let home = scratch();
        let layout = Layout::new(&home);
        let missing = "/run/media/x1/KeelMissingDrive/movies";
        let err = prepare_download_dir(&layout, missing).unwrap_err();
        assert!(err.contains("not mounted"), "{err}");
        assert!(!err.contains("os error"), "{err}");
        assert!(!err.contains("Permission denied"), "{err}");
        assert!(!Path::new("/run/media/x1/KeelMissingDrive").exists());
        let _ = fs::remove_dir_all("/run/media/x1/KeelMissingDrive");
        let _ = fs::remove_dir_all(&home);
    }
}
