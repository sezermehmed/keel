//! Folders and mounted drives the in-app browser can offer.
//!
//! The list is built from a mount table so a test never needs the real HDD.

use std::fs;
use std::path::{Path, PathBuf};

use crate::access;
use crate::layout::Layout;
use crate::source::{self, reject_links};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn places(layout: &Layout) -> Vec<Place> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    places_from(layout, &mounts)
}

pub fn places_from(layout: &Layout, mounts: &str) -> Vec<Place> {
    let mut out = vec![
        Place {
            label: "Home".into(),
            path: layout.home.clone(),
        },
        Place {
            label: "Downloads".into(),
            path: source::default_download_dir(layout),
        },
    ];
    for place in removable_places(mounts) {
        if !place.path.is_dir() || out.iter().any(|have| have.path == place.path) {
            continue;
        }
        out.push(place);
    }
    out
}

pub(crate) fn removable_places(mounts: &str) -> Vec<Place> {
    mounts_under_unchecked(mounts)
        .into_iter()
        .map(|(path, _kind)| {
            let label = path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Drive")
                .to_owned();
            Place { label, path }
        })
        .collect()
}

pub fn filesystem_kind(path: &Path) -> Option<String> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    kind_from(path, &mounts)
}

pub fn kind_from(path: &Path, mounts: &str) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for (mount, kind) in parse_mounts(mounts) {
        if path.starts_with(&mount) {
            let len = mount.as_os_str().len();
            if best.as_ref().is_none_or(|(have, _)| len > *have) {
                best = Some((len, kind));
            }
        }
    }
    best.map(|(_, kind)| kind)
}

pub fn fat32_warning(kind: Option<&str>) -> Option<&'static str> {
    match kind {
        Some("vfat" | "msdos" | "fat") => Some(
            "This drive is FAT32. A single file cannot be larger than 4 GB. A torrent with a bigger file will fail here.",
        ),
        _ => None,
    }
}

pub fn list_dir(path: &Path) -> Result<Vec<Entry>, String> {
    reject_links(path)?;
    let meta = fs::symlink_metadata(path).map_err(|err| access::io_sentence("open", path, &err))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!("{} is not a real folder.", path.display()));
    }
    let read = fs::read_dir(path).map_err(|err| access::io_sentence("open", path, &err))?;
    let mut entries = Vec::new();
    for item in read {
        let item = match item {
            Ok(item) => item,
            Err(_) => continue,
        };
        let name = item.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let meta = match fs::symlink_metadata(item.path()) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        entries.push(Entry {
            name: name.to_owned(),
            path: item.path(),
            is_dir: meta.is_dir(),
        });
    }
    entries.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

fn mounts_under_unchecked(mounts: &str) -> Vec<(PathBuf, String)> {
    parse_mounts(mounts)
        .into_iter()
        .filter(|(path, _)| {
            path.starts_with("/run/media") || path.starts_with("/media") || path.starts_with("/mnt")
        })
        .collect()
}

fn parse_mounts(mounts: &str) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(_device) = fields.next() else {
            continue;
        };
        let Some(point) = fields.next() else {
            continue;
        };
        let Some(kind) = fields.next() else {
            continue;
        };
        let point = unescape_mount(point);
        if point.is_empty() {
            continue;
        }
        out.push((PathBuf::from(point), kind.to_owned()));
    }
    out
}

fn unescape_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = std::str::from_utf8(&bytes[index + 1..index + 4]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(octal, 8) {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    const MOUNTS: &str = "\
/dev/nvme0n1p2 / ext4 rw 0 0
/dev/sda /run/media/x1/HDD vfat rw 0 0
/dev/mmcblk0p1 /run/media/x1/disk exfat rw 0 0
/dev/sda1 /mnt/Movies\\040Disk ext4 rw 0 0
";

    #[test]
    fn fat32_is_warned_and_exfat_is_not() {
        assert!(fat32_warning(Some("vfat")).is_some());
        assert!(fat32_warning(Some("exfat")).is_none());
        assert!(fat32_warning(Some("ext4")).is_none());
        assert_eq!(
            kind_from(Path::new("/run/media/x1/HDD/movies"), MOUNTS).as_deref(),
            Some("vfat")
        );
        assert_eq!(
            kind_from(Path::new("/run/media/x1/disk"), MOUNTS).as_deref(),
            Some("exfat")
        );
        assert_eq!(
            kind_from(Path::new("/mnt/Movies Disk"), MOUNTS).as_deref(),
            Some("ext4")
        );
    }

    #[test]
    fn places_include_removable_drives_and_skip_the_system_disk() {
        let home = std::env::temp_dir();
        let layout = Layout::new(&home);
        let places = places_from(&layout, MOUNTS);
        assert!(places.iter().any(|place| place.label == "Home"));
        let drives = removable_places(MOUNTS);
        assert!(drives.iter().any(|place| place.label == "HDD"));
        assert!(drives.iter().any(|place| place.path.ends_with("disk")));
        assert!(drives.iter().any(|place| place.label == "Movies Disk"));
        assert!(!drives.iter().any(|place| place.path == Path::new("/")));
    }

    #[test]
    fn a_symlink_is_not_listed() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("keel-places-{nanos}"));
        fs::create_dir_all(root.join("real")).unwrap();
        fs::write(root.join("note.txt"), b"x").unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        symlink("real", root.join("linked")).unwrap();
        let entries = list_dir(&root).unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.name == "real" && entry.is_dir));
        assert!(entries
            .iter()
            .any(|entry| entry.name == "note.txt" && !entry.is_dir));
        assert!(!entries.iter().any(|entry| entry.name == "linked"));
        assert!(!entries.iter().any(|entry| entry.name == ".hidden"));
        let _ = fs::remove_dir_all(&root);
    }
}
