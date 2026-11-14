//! Plain sentences for a drive that is not mounted and a folder Keel cannot write.
//!
//! The window asks to mount or to allow writing. It never shows errno text.

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Mount { label: String, volume: PathBuf },
    Write { path: PathBuf },
}

impl Problem {
    pub fn sentence(&self) -> String {
        match self {
            Problem::Mount { label, volume } => {
                format!("{label} is not mounted. {}", volume.display())
            }
            Problem::Write { path } => format!("Keel cannot write in {}.", path.display()),
        }
    }
}

/// `/run/media/<user>/<label>`, `/media/<user>/<label>`, or `/mnt/<label>`.
pub fn volume_of(path: &Path) -> Option<(String, PathBuf)> {
    if !path.is_absolute() {
        return None;
    }
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(name) => {
                let name = name.to_str()?;
                if name.is_empty() {
                    return None;
                }
                names.push(name);
            }
            _ => return None,
        }
    }
    if names.len() >= 4 && names[0] == "run" && names[1] == "media" {
        let label = names[3];
        return Some((
            label.to_owned(),
            PathBuf::from("/run/media").join(names[2]).join(label),
        ));
    }
    if names.len() >= 3 && names[0] == "media" {
        let label = names[2];
        return Some((
            label.to_owned(),
            PathBuf::from("/media").join(names[1]).join(label),
        ));
    }
    if names.len() >= 2 && names[0] == "mnt" {
        let label = names[1];
        return Some((label.to_owned(), PathBuf::from("/mnt").join(label)));
    }
    None
}

pub fn read_mounts() -> String {
    fs::read_to_string("/proc/mounts").unwrap_or_default()
}

pub fn inspect_live(path: &Path) -> Option<Problem> {
    inspect(path, &read_mounts())
}

/// `Mount` when `path` sits on a removable volume that is absent from `mounts`.
/// `Write` when an existing directory on the way cannot be written.
pub fn inspect(path: &Path, mounts: &str) -> Option<Problem> {
    if let Some((label, volume)) = volume_of(path) {
        if !is_mounted(&volume, mounts) {
            return Some(Problem::Mount { label, volume });
        }
    }
    let candidate = if path.is_dir() {
        path.to_path_buf()
    } else if path.exists() {
        return None;
    } else {
        let parent = path.parent()?;
        if parent.as_os_str().is_empty() || !parent.is_dir() {
            return None;
        }
        parent.to_path_buf()
    };
    if can_write(&candidate) {
        None
    } else {
        Some(Problem::Write { path: candidate })
    }
}

pub fn is_mounted(volume: &Path, mounts: &str) -> bool {
    mount_points(mounts)
        .into_iter()
        .any(|point| point.components().eq(volume.components()))
}

/// Give this account read, write, and enter on `path` only, then prove a file can be created.
pub fn allow_write(path: &Path) -> Result<(), String> {
    let refused = "This folder still cannot be written. Choose another folder in Browse.";
    let meta = fs::symlink_metadata(path).map_err(|_| refused.to_owned())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(refused.to_owned());
    }
    let mode = meta.permissions().mode() | 0o700;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| refused.to_owned())?;
    let probe = path.join(".keel-write-check");
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);
    let _ = fs::remove_file(&probe);
    if created.is_ok() {
        Ok(())
    } else {
        Err(refused.to_owned())
    }
}

/// Mount the removable disk with this label. No administrator password and no system dialog.
pub fn mount_drive(label: &str) -> Result<(), String> {
    if label.is_empty() || label.contains('/') || label.contains('\0') || label.starts_with('-') {
        return Err("Plug the drive in, then try again.".into());
    }
    let Some(device) = device_by_label(label) else {
        return Err(format!("Plug in {label}, then try again."));
    };
    let output = Command::new("udisksctl")
        .args(["mount", "-b", "--no-user-interaction"])
        .arg(&device)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(_) => return Err(format!("Plug in {label}, then try again.")),
    };
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    if text.to_ascii_lowercase().contains("already mounted") {
        return Ok(());
    }
    if text.is_empty() {
        return Err(format!("Plug in {label}, then try again."));
    }
    Err(plain(text))
}

pub fn explain(hint: &Path, message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return String::new();
    }
    if let Some(problem) = problem_from_sentence(message) {
        return problem.sentence();
    }
    if looks_like_access(message) {
        if let Some(problem) = inspect_live(hint) {
            return problem.sentence();
        }
        if let Some(problem) = embedded_problem(message) {
            return problem.sentence();
        }
    }
    plain(message)
}

pub fn problem_from_sentence(message: &str) -> Option<Problem> {
    let text = message.trim();
    if let Some(rest) = text.strip_prefix("Keel cannot write in ") {
        let path = strip_period(rest);
        if path.is_empty() {
            return None;
        }
        return Some(Problem::Write {
            path: PathBuf::from(path),
        });
    }
    let (label, volume) = text.split_once(" is not mounted.")?;
    let label = label.trim();
    let volume = strip_period(volume);
    if label.is_empty()
        || volume.is_empty()
        || label.contains('\n')
        || label.len() > 200
        || label.contains(" is not mounted")
    {
        return None;
    }
    Some(Problem::Mount {
        label: label.to_owned(),
        volume: PathBuf::from(volume),
    })
}

/// Drop errno names and "(os error N)" so the window never shows them.
pub fn plain(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        return String::new();
    }
    let mut text = strip_os_error(message);
    for phrase in [
        "permission denied",
        "operation not permitted",
        "read-only file system",
        "transport endpoint is not connected",
        "permissiondenied",
    ] {
        text = cut_phrase(&text, phrase);
    }
    for word in ["EACCES", "EPERM", "ENOENT", "EROFS", "ENOTCONN"] {
        text = cut_word(&text, word);
    }
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let collapsed = collapsed
        .trim()
        .trim_matches(|ch| matches!(ch, ':' | ',' | ';'));
    let collapsed = collapsed.trim();
    if collapsed.is_empty() {
        return "Keel could not do that.".into();
    }
    let mut out = collapsed.to_owned();
    while out.ends_with(':') || out.ends_with(',') {
        out.pop();
        out = out.trim().to_owned();
    }
    if out.is_empty() {
        return "Keel could not do that.".into();
    }
    if !out.ends_with('.') && !out.ends_with('!') && !out.ends_with('?') {
        out.push('.');
    }
    out
}

pub fn io_sentence(action: &str, path: &Path, err: &io::Error) -> String {
    explain(
        path,
        &format!("Could not {action} {}: {err}", path.display()),
    )
}

fn embedded_problem(message: &str) -> Option<Problem> {
    if let Some(index) = message.find("Keel cannot write in ") {
        if let Some(problem) = problem_from_sentence(&message[index..]) {
            return Some(problem);
        }
    }
    let index = message.find(" is not mounted.")?;
    let start = message[..index]
        .rfind([':', '.'])
        .map(|mark| mark + 1)
        .unwrap_or(0);
    problem_from_sentence(message[start..].trim())
}

fn looks_like_access(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("permission")
        || lower.contains("os error")
        || lower.contains("eacces")
        || lower.contains("eperm")
        || lower.contains("erofs")
        || lower.contains("read-only")
        || lower.contains("not permitted")
        || lower.contains("not mounted")
        || lower.contains("cannot write")
}

fn can_write(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // Safety: `c_path` is a nul-terminated path. `access` only reads it.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

fn device_by_label(label: &str) -> Option<PathBuf> {
    let dir = Path::new("/dev/disk/by-label");
    let direct = dir.join(label);
    if direct.exists() {
        return Some(direct);
    }
    let encoded = dir.join(encode_udev(label));
    if encoded.exists() {
        return Some(encoded);
    }
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if decode_udev(&name) == label {
            return Some(entry.path());
        }
    }
    None
}

fn encode_udev(label: &str) -> String {
    let mut out = String::new();
    for byte in label.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
    out
}

fn decode_udev(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() && bytes[index + 1] == b'x' {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 2..index + 4]).unwrap_or(""),
                16,
            ) {
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

fn mount_points(mounts: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(_device) = fields.next() else {
            continue;
        };
        let Some(point) = fields.next() else {
            continue;
        };
        let point = unescape_mount(point);
        if !point.is_empty() {
            out.push(PathBuf::from(point));
        }
    }
    out
}

fn unescape_mount(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 4]).unwrap_or(""),
                8,
            ) {
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

fn strip_period(text: &str) -> &str {
    text.trim().strip_suffix('.').unwrap_or(text.trim()).trim()
}

fn strip_os_error(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("(os error ") {
        let after = &rest[start..];
        let Some(end) = after.find(')') else {
            out.push_str(rest);
            return out;
        };
        let mut cut = start;
        if cut > 0 && rest.as_bytes()[cut - 1] == b' ' {
            cut -= 1;
        }
        out.push_str(&rest[..cut]);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

fn cut_phrase(text: &str, phrase: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let phrase = phrase.to_ascii_lowercase();
    let mut out = String::new();
    let mut rest = text;
    let mut lower_rest = lower.as_str();
    while let Some(index) = lower_rest.find(phrase.as_str()) {
        out.push_str(&rest[..index]);
        let next = index + phrase.len();
        rest = &rest[next..];
        lower_rest = &lower_rest[next..];
    }
    out.push_str(rest);
    out
}

fn cut_word(text: &str, word: &str) -> String {
    let mut out = String::new();
    for token in text.split_inclusive(char::is_whitespace) {
        if token.trim().eq_ignore_ascii_case(word) {
            continue;
        }
        out.push_str(token);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn volume_of_a_removable_folder() {
        let (label, volume) = volume_of(Path::new("/run/media/x1/HDD/movies")).unwrap();
        assert_eq!(label, "HDD");
        assert_eq!(volume, PathBuf::from("/run/media/x1/HDD"));
        let (label, volume) = volume_of(Path::new("/media/x1/disk")).unwrap();
        assert_eq!(label, "disk");
        assert_eq!(volume, PathBuf::from("/media/x1/disk"));
        let (label, volume) = volume_of(Path::new("/mnt/Movies Disk/a")).unwrap();
        assert_eq!(label, "Movies Disk");
        assert_eq!(volume, PathBuf::from("/mnt/Movies Disk"));
        assert!(volume_of(Path::new("/home/x1/Downloads")).is_none());
        assert!(volume_of(Path::new("/run/media/x1")).is_none());
    }

    #[test]
    fn a_missing_mount_is_a_mount_problem() {
        let mounts = "\
/dev/nvme0n1p2 / ext4 rw 0 0
/dev/sda /mnt/KeelMissingDrive ext4 rw 0 0
";
        let missing = inspect(Path::new("/run/media/x1/HDD/movies"), mounts).unwrap();
        assert_eq!(
            missing,
            Problem::Mount {
                label: "HDD".into(),
                volume: PathBuf::from("/run/media/x1/HDD"),
            }
        );
        let mounted = inspect(Path::new("/mnt/KeelMissingDrive/movies"), mounts);
        assert!(!matches!(mounted, Some(Problem::Mount { .. })));
        let escaped = "\
/dev/sda /mnt/Movies\\040Disk ext4 rw 0 0
";
        assert!(is_mounted(Path::new("/mnt/Movies Disk"), escaped));
    }

    #[test]
    fn a_locked_folder_can_be_opened_for_this_account() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("keel-access-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let problem = inspect(&dir, "").unwrap();
        assert_eq!(problem, Problem::Write { path: dir.clone() });
        allow_write(&dir).unwrap();
        assert!(can_write(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_hides_errno_text() {
        let shown = plain("Permission denied (os error 13)");
        assert!(!shown.contains("os error"), "{shown}");
        assert!(
            !shown.to_ascii_lowercase().contains("permission denied"),
            "{shown}"
        );
        let sentence = explain(
            Path::new("/run/media/x1/KeelMissingDrive/movies"),
            "Could not create /run/media/x1/KeelMissingDrive/movies: Permission denied (os error 13)",
        );
        assert!(sentence.contains("not mounted"), "{sentence}");
        assert!(!sentence.contains("os error"), "{sentence}");
        assert!(!sentence.contains("Permission denied"), "{sentence}");
        let parsed = problem_from_sentence(&sentence).unwrap();
        assert_eq!(
            parsed,
            Problem::Mount {
                label: "KeelMissingDrive".into(),
                volume: PathBuf::from("/run/media/x1/KeelMissingDrive"),
            }
        );
        assert_eq!(decode_udev(r"My\x20Disk"), "My Disk");
        assert_eq!(encode_udev("My Disk"), r"My\x20Disk");
    }
}
