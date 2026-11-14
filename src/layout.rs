//! Where a Linux user-account install keeps its files.

use std::env;
use std::path::{Component, Path, PathBuf};

pub const APP_ID: &str = "keel";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub home: PathBuf,
    pub data_home: PathBuf,
    pub config_home: PathBuf,
}

impl Layout {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let home = if home.is_dir() {
            home.canonicalize().unwrap_or(home)
        } else {
            home
        };
        Self {
            data_home: home.join(".local/share"),
            config_home: home.join(".config"),
            home,
        }
    }

    /// `KEEL_HOME` replaces the home directory and ignores `XDG_*`, so a trial
    /// install stays inside that folder. Otherwise the XDG base directories
    /// are used when they are absolute and do not contain `..`.
    pub fn from_env() -> Result<Self, String> {
        if let Some(over) = env::var_os("KEEL_HOME") {
            let home = PathBuf::from(over);
            if !home.is_absolute() {
                return Err("KEEL_HOME must be a full path.".into());
            }
            if !home.is_dir() {
                return Err(format!("KEEL_HOME is not a folder: {}", home.display()));
            }
            return Ok(Self::new(home));
        }

        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set.".to_owned())?;
        if !home.is_dir() {
            return Err(format!("HOME is not a folder: {}", home.display()));
        }
        let mut layout = Self::new(home);
        if let Some(data) = absolute_xdg("XDG_DATA_HOME")? {
            layout.data_home = data;
        }
        if let Some(config) = absolute_xdg("XDG_CONFIG_HOME")? {
            layout.config_home = config;
        }
        Ok(layout)
    }

    pub fn default_install_dir(&self) -> PathBuf {
        self.data_home.join(APP_ID)
    }

    pub fn applications_dir(&self) -> PathBuf {
        self.data_home.join("applications")
    }

    pub fn desktop_path(&self) -> PathBuf {
        self.applications_dir().join("keel.desktop")
    }

    pub fn mimeapps_path(&self) -> PathBuf {
        self.config_home.join("mimeapps.list")
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    pub fn bin_link(&self) -> PathBuf {
        self.bin_dir().join(APP_ID)
    }

    /// Engine socket, lock, and inbox. Kept under the data directory so a
    /// `KEEL_HOME` trial never writes into the real user's runtime directory.
    pub fn engine_dir(&self) -> PathBuf {
        self.data_home.join(APP_ID).join("engine")
    }

    pub fn session_dir(&self) -> PathBuf {
        self.data_home.join(APP_ID).join("session")
    }

    pub fn socket_path(&self) -> PathBuf {
        self.engine_dir().join("engine.sock")
    }

    pub fn inbox_path(&self) -> PathBuf {
        self.engine_dir().join("inbox")
    }

    pub fn ui_lock_path(&self) -> PathBuf {
        self.engine_dir().join("ui.lock")
    }

    pub fn engine_log_path(&self) -> PathBuf {
        self.engine_dir().join("engine.log")
    }

    /// Remembered save folder. Uninstall leaves this file in place.
    pub fn settings_path(&self) -> PathBuf {
        self.data_home.join(APP_ID).join("settings.txt")
    }
}

fn absolute_xdg(name: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Ok(None);
    }
    if path.exists() {
        return path.canonicalize().map(Some).map_err(|err| {
            crate::access::explain(&path, &format!("Could not open {name}: {err}"))
        });
    }
    Ok(Some(path))
}

pub fn dir_on_path(dir: &Path, path_var: Option<&str>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    let canon = dir.canonicalize().ok();
    path_var
        .split(':')
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = Path::new(entry);
            entry == dir
                || canon
                    .as_ref()
                    .is_some_and(|canon| entry.canonicalize().ok().as_ref() == Some(canon))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_match_accepts_the_same_folder() {
        let home = PathBuf::from("/home/example");
        assert!(dir_on_path(
            &home.join(".local/bin"),
            Some("/usr/bin:/home/example/.local/bin:/bin")
        ));
        assert!(!dir_on_path(
            &home.join(".local/bin"),
            Some("/usr/bin:/bin")
        ));
        assert!(!dir_on_path(&home.join(".local/bin"), None));
    }

    #[test]
    fn engine_files_stay_under_the_data_directory() {
        let layout = Layout::new("/keel-layout-test-home");
        assert_eq!(
            layout.socket_path(),
            PathBuf::from("/keel-layout-test-home/.local/share/keel/engine/engine.sock")
        );
        assert_eq!(
            layout.session_dir(),
            PathBuf::from("/keel-layout-test-home/.local/share/keel/session")
        );
    }
}
