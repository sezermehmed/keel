//! The save folder Keel should offer next time.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::access;
use crate::layout::Layout;
use crate::source;

const KEY: &str = "download_dir";

/// The absolute path last saved, even when that drive is not mounted.
pub fn remembered_download_dir(layout: &Layout) -> Option<PathBuf> {
    let text = fs::read_to_string(layout.settings_path()).ok()?;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == KEY {
            let path = PathBuf::from(value.trim());
            if path.is_absolute() && !path.as_os_str().is_empty() {
                return Some(path);
            }
        }
    }
    None
}

pub fn load_download_dir(layout: &Layout) -> Option<PathBuf> {
    remembered_download_dir(layout).filter(|path| path.is_dir())
}

pub fn save_download_dir(layout: &Layout, path: &Path) -> Result<(), String> {
    let file = layout.settings_path();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|err| access::io_sentence("create", parent, &err))?;
    }
    let tmp = file.with_extension("txt.new");
    let mut out = fs::File::create(&tmp).map_err(|err| access::io_sentence("write", &tmp, &err))?;
    writeln!(out, "{KEY}={}", path.display())
        .map_err(|err| access::io_sentence("write", &tmp, &err))?;
    out.sync_all()
        .map_err(|err| access::io_sentence("write", &tmp, &err))?;
    fs::rename(&tmp, &file).map_err(|err| access::io_sentence("write", &file, &err))?;
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
    Ok(())
}

/// The folder typed into Save to when the window opens.
pub fn initial_download_dir(layout: &Layout) -> PathBuf {
    load_download_dir(layout).unwrap_or_else(|| source::default_download_dir(layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn a_remembered_folder_is_read_back() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-settings-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::new(&home);
        let folder = home.join("movies");
        fs::create_dir_all(&folder).unwrap();
        save_download_dir(&layout, &folder).unwrap();
        assert_eq!(load_download_dir(&layout).unwrap(), folder);
        assert_eq!(initial_download_dir(&layout), folder);
        let missing = home.join("not-created");
        save_download_dir(&layout, &missing).unwrap();
        assert_eq!(remembered_download_dir(&layout).unwrap(), missing);
        assert!(load_download_dir(&layout).is_none());
        assert_eq!(
            initial_download_dir(&layout),
            source::default_download_dir(&layout)
        );
        let _ = fs::remove_dir_all(&home);
    }
}
