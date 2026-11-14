//! User-account install and uninstall for Linux.
//!
//! Files are written under the home directory. Uninstall deletes the program,
//! the icon, the menu entry Keel wrote, and its own command link. It does not
//! delete downloaded files, and it does not remove a folder that still holds
//! something else.

use std::fs;
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use crate::icon;
use crate::layout::{self, Layout};
use crate::mime;
use crate::wire;

pub const MIME_TYPES: [&str; 2] = ["application/x-bittorrent", "x-scheme-handler/magnet"];
const DESKTOP_ID: &str = "keel.desktop";
const DESKTOP_MARKER: &str = "# keel-install";
const BINARY_NAME: &str = "keel";
const ICON_NAME: &str = "keel.png";
const MANIFEST_NAME: &str = "install.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub layout: Layout,
    pub source_binary: PathBuf,
    pub install_dir: PathBuf,
    pub register_handlers: bool,
    pub add_to_path: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    pub binary: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallError {
    NotInstalled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Manifest {
    version: String,
    binary: PathBuf,
    icon: PathBuf,
    desktop: PathBuf,
    symlink: Option<PathBuf>,
    handlers: bool,
    previous: [Option<String>; 2],
}

struct Found {
    dir: PathBuf,
    manifest: Manifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkStatus {
    Created,
    AlreadyCorrect,
    SkippedForeign,
    NotRequested,
}

pub fn validate_install_dir(layout: &Layout, input: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(&layout.home, input.trim());
    if expanded.as_os_str().is_empty() {
        return Err("Choose a folder inside your home directory.".into());
    }
    check_dir(layout, &expanded)
}

pub fn install(request: &Request) -> Result<Report, String> {
    let dir = check_dir(&request.layout, &request.install_dir)?;
    let source = fs::metadata(&request.source_binary)
        .map_err(|err| io_at("read the Keel program at", &request.source_binary, err))?;
    if !source.is_file() {
        return Err(format!(
            "{} is not a program file.",
            request.source_binary.display()
        ));
    }

    fs::create_dir_all(&dir).map_err(|err| io_at("create", &dir, err))?;
    let binary = dir.join(BINARY_NAME);
    let icon = dir.join(ICON_NAME);
    copy_program(&request.source_binary, &binary)?;
    atomic_write(&icon, &icon::png_bytes(128))?;

    let desktop = request.layout.desktop_path();
    atomic_write(&desktop, desktop_file(&binary, &icon).as_bytes())?;

    let existing = read_found(&request.layout, &dir);
    let handlers_were_on = existing
        .as_ref()
        .is_some_and(|found| found.manifest.handlers);
    let mut previous = [None, None];
    let mut handler_line: String;
    let mut revoked = false;

    if request.register_handlers {
        let mime_path = request.layout.mimeapps_path();
        let original = read_text_optional(&mime_path)?;
        let mut current = mime::defaults_for(&original, &MIME_TYPES);
        if let Some(found) = &existing {
            for (index, value) in current.iter_mut().enumerate() {
                if value.as_deref() == Some(DESKTOP_ID) {
                    *value = found.manifest.previous[index].clone();
                }
            }
        }
        previous = [current[0].clone(), current[1].clone()];
        let updated = mime::grant(&original, DESKTOP_ID, &MIME_TYPES);
        atomic_write(&mime_path, updated.as_bytes())?;
        handler_line = "Torrent files and magnet links now open with Keel.".into();
    } else if handlers_were_on {
        let found = existing.as_ref().expect("handlers were recorded");
        revoke_handlers(&request.layout, &found.manifest)?;
        revoked = true;
        handler_line =
            "Keel is no longer the program that opens torrent files and magnet links.".into();
    } else {
        handler_line = "Torrent files and magnet links were left unchanged.".into();
    }

    let link_status = if request.add_to_path {
        ensure_link(&request.layout, &binary)?
    } else {
        if let Some(found) = &existing {
            if found.manifest.symlink.is_some() {
                remove_our_link(&request.layout, &binary)?;
            }
        }
        LinkStatus::NotRequested
    };

    let manifest = Manifest {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        binary: binary.clone(),
        icon: icon.clone(),
        desktop: desktop.clone(),
        symlink: match link_status {
            LinkStatus::Created | LinkStatus::AlreadyCorrect => Some(request.layout.bin_link()),
            LinkStatus::SkippedForeign | LinkStatus::NotRequested => None,
        },
        handlers: request.register_handlers,
        previous,
    };
    write_manifest(&dir.join(MANIFEST_NAME), &manifest)?;
    if let Some(note) = refresh_menus(&request.layout.applications_dir()) {
        handler_line = if handler_line.is_empty() {
            note
        } else {
            format!("{handler_line} {note}")
        };
    }

    let mut lines = vec![
        format!("Program copied to {}.", binary.display()),
        format!("Menu entry written to {}.", desktop.display()),
        handler_line,
        path_note(&request.layout, link_status),
    ];
    if revoked {
        lines.retain(|line| !line.is_empty());
    }
    Ok(Report { lines, binary })
}

pub fn uninstall(layout: &Layout) -> Result<Report, UninstallError> {
    let mut lines = Vec::new();
    match wire::shutdown(layout) {
        Ok(true) => lines.push("Stopped the download engine.".into()),
        Ok(false) => {}
        Err(message) => lines.push(format!("The download engine did not stop: {message}")),
    }

    let Some(found) = locate_found(layout) else {
        return Err(UninstallError::NotInstalled);
    };
    let binary = found.dir.join(BINARY_NAME);

    if found.manifest.handlers {
        revoke_handlers(layout, &found.manifest).map_err(UninstallError::Failed)?;
    }

    remove_desktop_if_ours(layout, &found).map_err(UninstallError::Failed)?;
    lines.push("Removed the menu entry.".into());

    remove_if_present(&found.dir.join(ICON_NAME)).map_err(UninstallError::Failed)?;
    if binary.is_file() {
        remove_if_present(&binary).map_err(UninstallError::Failed)?;
        lines.push(format!("Removed the program at {}.", binary.display()));
    } else if binary.exists() {
        remove_if_present(&binary).map_err(UninstallError::Failed)?;
    } else {
        lines.push("The program file was already gone.".into());
    }

    if found.manifest.handlers {
        lines.push(
            "Restored file associations to what they were before Keel took them over.".into(),
        );
    } else {
        lines.push("File associations were left unchanged.".into());
    }

    if found.manifest.symlink.is_some() {
        match remove_our_link(layout, &binary) {
            Ok(true) => lines.push(format!(
                "Removed the command link at {}.",
                layout.bin_link().display()
            )),
            Ok(false) => lines.push(format!(
                "Left {} alone because it does not point at Keel.",
                layout.bin_link().display()
            )),
            Err(message) => return Err(UninstallError::Failed(message)),
        }
    }

    remove_if_present(&found.dir.join(MANIFEST_NAME)).map_err(UninstallError::Failed)?;
    if directory_is_empty(&found.dir).map_err(UninstallError::Failed)? {
        fs::remove_dir(&found.dir)
            .map_err(|err| UninstallError::Failed(io_at("remove", &found.dir, err)))?;
        lines.push(format!("Removed the folder {}.", found.dir.display()));
    } else {
        lines.push(format!(
            "Left {} in place because it still holds other files.",
            found.dir.display()
        ));
    }
    lines.push("Downloaded files were not touched.".into());
    let _ = refresh_menus(&layout.applications_dir());

    Ok(Report { lines, binary })
}

pub fn locate(layout: &Layout) -> Option<PathBuf> {
    locate_found(layout).map(|found| found.dir)
}

/// True when the running program sits next to an install manifest.
pub fn running_from_install() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    read_manifest(&dir.join(MANIFEST_NAME))
        .is_some_and(|manifest| manifest.binary == dir.join(BINARY_NAME))
}

fn locate_found(layout: &Layout) -> Option<Found> {
    if let Some(found) = locate_from_desktop(layout) {
        return Some(found);
    }
    if let Some(found) = read_found(layout, &layout.default_install_dir()) {
        return Some(found);
    }
    locate_from_symlink(layout)
}

fn locate_from_desktop(layout: &Layout) -> Option<Found> {
    let text = fs::read_to_string(layout.desktop_path()).ok()?;
    if !has_marker(&text) {
        return None;
    }
    let binary = parse_exec(&text)?;
    let parent = binary.parent()?.to_path_buf();
    read_found(layout, &parent)
}

fn locate_from_symlink(layout: &Layout) -> Option<Found> {
    let target = fs::read_link(layout.bin_link()).ok()?;
    let absolute = if target.is_absolute() {
        target
    } else {
        layout.bin_dir().join(target)
    };
    let parent = absolute.parent()?.to_path_buf();
    read_found(layout, &parent)
}

fn read_found(layout: &Layout, dir: &Path) -> Option<Found> {
    let dir = dir.canonicalize().ok()?;
    if !dir.starts_with(&layout.home) || dir == layout.home {
        return None;
    }
    let manifest = read_manifest(&dir.join(MANIFEST_NAME))?;
    if manifest.binary != dir.join(BINARY_NAME) || manifest.icon != dir.join(ICON_NAME) {
        return None;
    }
    Some(Found { dir, manifest })
}

fn check_dir(layout: &Layout, folder: &Path) -> Result<PathBuf, String> {
    if !folder.is_absolute() {
        return Err("Use a full path inside your home directory.".into());
    }
    if folder
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The folder path cannot contain '..'.".into());
    }
    let Some(text) = folder.to_str() else {
        return Err("The folder path is not valid text.".into());
    };
    if text.contains(['\n', '\r', '"', '\0']) {
        return Err("The folder path cannot contain quotes or line breaks.".into());
    }
    reject_links_along(folder)?;

    let resolved = if folder.exists() {
        let meta = fs::symlink_metadata(folder).map_err(|err| io_at("inspect", folder, err))?;
        if !meta.is_dir() {
            return Err("That path is a file, not a folder.".into());
        }
        folder
            .canonicalize()
            .map_err(|err| io_at("open", folder, err))?
    } else {
        let ancestor = nearest_existing_ancestor(folder)?;
        let canon = ancestor
            .canonicalize()
            .map_err(|err| io_at("open", &ancestor, err))?;
        if !canon.starts_with(&layout.home) {
            return Err(outside_home());
        }
        let rest = folder
            .strip_prefix(&ancestor)
            .map_err(|_| "Could not resolve the folder.".to_owned())?;
        canon.join(rest)
    };
    accept_dir(layout, resolved)
}

fn accept_dir(layout: &Layout, folder: PathBuf) -> Result<PathBuf, String> {
    if !folder.starts_with(&layout.home) {
        return Err(outside_home());
    }
    if folder == layout.home {
        return Err(
            "Choose a folder inside your home directory, not the home directory itself.".into(),
        );
    }
    let reserved = [
        layout.home.join(".local"),
        layout.bin_dir(),
        layout.data_home.clone(),
        layout.applications_dir(),
        layout.config_home.clone(),
    ];
    if reserved.iter().any(|path| path == &folder) {
        return Err(
            "Choose a folder that is only for Keel. That one is used by the system.".into(),
        );
    }
    Ok(folder)
}

fn outside_home() -> String {
    "The folder has to be inside your home directory, so the install does not need an administrator password.".into()
}

fn reject_links_along(folder: &Path) -> Result<(), String> {
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
            Err(err) => return Err(io_at("inspect", &current, err)),
        }
    }
    Ok(())
}

fn nearest_existing_ancestor(folder: &Path) -> Result<PathBuf, String> {
    let mut current = folder.to_path_buf();
    while !current.exists() {
        if !current.pop() {
            return Err("That folder cannot be reached.".into());
        }
    }
    Ok(current)
}

fn expand_tilde(home: &Path, input: &str) -> PathBuf {
    if input == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.join(rest);
    }
    PathBuf::from(input)
}

fn copy_program(source: &Path, dest: &Path) -> Result<(), String> {
    if same_file(source, dest) {
        make_executable(dest)?;
        return Ok(());
    }
    let parent = dest
        .parent()
        .ok_or("The program destination has no folder.")?;
    let temporary = parent.join("keel.new");
    fs::copy(source, &temporary).map_err(|err| {
        let _ = fs::remove_file(&temporary);
        io_at("copy the program to", dest, err)
    })?;
    if let Err(err) = make_executable(&temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    fs::rename(&temporary, dest).map_err(|err| {
        let _ = fs::remove_file(&temporary);
        io_at("place the program at", dest, err)
    })
}

fn make_executable(path: &Path) -> Result<(), String> {
    let mut permissions = fs::metadata(path)
        .map_err(|err| io_at("update", path, err))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).map_err(|err| io_at("update", path, err))
}

fn ensure_link(layout: &Layout, binary: &Path) -> Result<LinkStatus, String> {
    let link = layout.bin_link();
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(|err| io_at("create", parent, err))?;
    }
    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() && links_to(&link, binary) => {
            Ok(LinkStatus::AlreadyCorrect)
        }
        Ok(_) => Ok(LinkStatus::SkippedForeign),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            symlink(binary, &link)
                .map_err(|err| io_at("create the command link at", &link, err))?;
            Ok(LinkStatus::Created)
        }
        Err(err) => Err(io_at("inspect", &link, err)),
    }
}

/// Removes the command link when it points at `binary`. Returns whether it was removed.
fn remove_our_link(layout: &Layout, binary: &Path) -> Result<bool, String> {
    let link = layout.bin_link();
    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() && links_to(&link, binary) => {
            fs::remove_file(&link).map_err(|err| io_at("remove", &link, err))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io_at("inspect", &link, err)),
    }
}

fn links_to(link: &Path, binary: &Path) -> bool {
    let Ok(target) = fs::read_link(link) else {
        return false;
    };
    let absolute = if target.is_absolute() {
        target
    } else {
        link.parent().unwrap_or(Path::new("/")).join(target)
    };
    absolute == binary || same_file(&absolute, binary)
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn path_note(layout: &Layout, status: LinkStatus) -> String {
    match status {
        LinkStatus::Created | LinkStatus::AlreadyCorrect => {
            if layout::dir_on_path(&layout.bin_dir(), std::env::var("PATH").ok().as_deref()) {
                format!(
                    "The keel command is on PATH at {}.",
                    layout.bin_link().display()
                )
            } else {
                format!(
                    "Created {}. That folder is not on PATH in this session, so the menu entry is the way to open Keel.",
                    layout.bin_link().display()
                )
            }
        }
        LinkStatus::SkippedForeign => format!(
            "Left {} alone because it already exists and does not point at this Keel.",
            layout.bin_link().display()
        ),
        LinkStatus::NotRequested => "No command was added to PATH.".into(),
    }
}

fn revoke_handlers(layout: &Layout, manifest: &Manifest) -> Result<(), String> {
    let path = layout.mimeapps_path();
    let original = read_text_optional(&path)?;
    let updated = mime::revoke(&original, DESKTOP_ID, &MIME_TYPES, &manifest.previous);
    if updated == original {
        return Ok(());
    }
    atomic_write(&path, updated.as_bytes())
}

fn remove_desktop_if_ours(layout: &Layout, found: &Found) -> Result<(), String> {
    let mut paths = vec![layout.desktop_path(), found.manifest.desktop.clone()];
    paths.sort();
    paths.dedup();
    for path in paths {
        if !desktop_is_deletable(layout, &found.dir, &path)? {
            continue;
        }
        remove_regular(&path)?;
    }
    Ok(())
}

fn desktop_is_deletable(layout: &Layout, install_dir: &Path, path: &Path) -> Result<bool, String> {
    if path.file_name().and_then(|name| name.to_str()) != Some(DESKTOP_ID) {
        return Ok(false);
    }
    if path.parent().and_then(|parent| parent.file_name())
        != Some(std::ffi::OsStr::new("applications"))
    {
        return Ok(false);
    }
    let under_home = path.starts_with(&layout.home);
    let under_data = path.starts_with(&layout.data_home);
    if !under_home && !under_data {
        return Ok(false);
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(io_at("read", path, err)),
    };
    if !has_marker(&text) {
        return Ok(false);
    }
    let Some(binary) = parse_exec(&text) else {
        return Ok(false);
    };
    Ok(binary.parent() == Some(install_dir))
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match remove_regular(path) {
        Err(message) if message.contains("already gone") => Ok(()),
        other => other,
    }
}

fn remove_regular(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "Refused to remove {} because it is a link.",
            path.display()
        )),
        Ok(meta) if meta.is_file() => {
            fs::remove_file(path).map_err(|err| io_at("remove", path, err))
        }
        Ok(_) => Err(format!(
            "Refused to remove {} because it is not a file.",
            path.display()
        )),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Err(format!("{} is already gone.", path.display()))
        }
        Err(err) => Err(io_at("inspect", path, err)),
    }
}

fn directory_is_empty(path: &Path) -> Result<bool, String> {
    let mut entries = fs::read_dir(path).map_err(|err| io_at("read", path, err))?;
    Ok(entries.next().is_none())
}

fn desktop_file(binary: &Path, icon: &Path) -> String {
    format!(
        "{DESKTOP_MARKER}\n\
[Desktop Entry]\n\
Type=Application\n\
Version=1.5\n\
Name=Keel\n\
GenericName=Torrent client\n\
Comment=Download torrents\n\
Exec=\"{binary}\" %U\n\
Icon={icon}\n\
Terminal=false\n\
Categories=Network;FileTransfer;\n\
MimeType=application/x-bittorrent;x-scheme-handler/magnet;\n\
StartupWMClass=keel\n",
        binary = binary.display(),
        icon = icon.display(),
    )
}

fn has_marker(text: &str) -> bool {
    text.lines().any(|line| line.trim() == DESKTOP_MARKER)
}

fn parse_exec(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("Exec=") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            let path = PathBuf::from(&quoted[..end]);
            if path.is_absolute() {
                return Some(path);
            }
            return None;
        }
        let token = rest.split_whitespace().next()?;
        let path = PathBuf::from(token);
        if path.is_absolute() {
            return Some(path);
        }
        return None;
    }
    None
}

fn refresh_menus(applications: &Path) -> Option<String> {
    match Command::new("update-desktop-database")
        .arg(applications)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => None,
        Ok(_) => Some("The menu cache command failed. The menu entry is still in place.".into()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(_) => {
            Some("Could not refresh the menu cache. The menu entry is still in place.".into())
        }
    }
}

fn read_text_optional(path: &Path) -> Result<String, String> {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| {
            format!(
                "{} is not UTF-8, so Keel left it unchanged.",
                path.display()
            )
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(io_at("read", path, err)),
    }
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let mut lines = vec![
        "id=keel".to_owned(),
        format!("version={}", manifest.version),
        format!("binary={}", display_path(&manifest.binary)?),
        format!("icon={}", display_path(&manifest.icon)?),
        format!("desktop={}", display_path(&manifest.desktop)?),
        format!("handlers={}", if manifest.handlers { "yes" } else { "no" }),
    ];
    if let Some(link) = &manifest.symlink {
        lines.push(format!("symlink={}", display_path(link)?));
    }
    for (mime, previous) in MIME_TYPES.iter().zip(manifest.previous.iter()) {
        let value = previous.as_deref().unwrap_or("");
        if value.contains(['\n', '\r', '=']) {
            return Err("A stored file association is not usable.".into());
        }
        lines.push(format!("previous.{mime}={value}"));
    }
    lines.push(String::new());
    atomic_write(path, lines.join("\n").as_bytes())
}

fn read_manifest(path: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(path).ok()?;
    let mut id = None;
    let mut version = None;
    let mut binary = None;
    let mut icon = None;
    let mut desktop = None;
    let mut symlink = None;
    let mut handlers = false;
    let mut previous = [None, None];
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "id" => id = Some(value.to_owned()),
            "version" => version = Some(value.to_owned()),
            "binary" => binary = Some(PathBuf::from(value)),
            "icon" => icon = Some(PathBuf::from(value)),
            "desktop" => desktop = Some(PathBuf::from(value)),
            "symlink" if !value.is_empty() => symlink = Some(PathBuf::from(value)),
            "handlers" => handlers = value == "yes",
            other => {
                if let Some(mime) = other.strip_prefix("previous.") {
                    if let Some(index) = MIME_TYPES.iter().position(|item| *item == mime) {
                        previous[index] = if value.is_empty() {
                            None
                        } else {
                            Some(value.to_owned())
                        };
                    }
                }
            }
        }
    }
    if id.as_deref() != Some("keel") {
        return None;
    }
    Some(Manifest {
        version: version.unwrap_or_else(|| "0".to_owned()),
        binary: binary?,
        icon: icon?,
        desktop: desktop?,
        symlink,
        handlers,
        previous,
    })
}

fn display_path(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("A path is not valid text.")?.to_owned();
    if text.contains(['\n', '\r']) {
        return Err("A path contains a line break.".into());
    }
    Ok(text)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("Could not write {}.", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| io_at("create", parent, err))?;
    let mut temporary_name = path
        .file_name()
        .ok_or_else(|| format!("Could not write {}.", path.display()))?
        .to_os_string();
    temporary_name.push(".keel-tmp");
    let temporary = parent.join(temporary_name);
    if let Err(err) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(io_at("write", path, err));
    }
    if let Err(err) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(io_at("write", path, err));
    }
    Ok(())
}

fn io_at(action: &str, path: &Path, err: io::Error) -> String {
    crate::access::io_sentence(action, path, &err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("keel-{name}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn layout(&self) -> Layout {
            Layout::new(&self.0)
        }

        fn source(&self, bytes: &[u8]) -> PathBuf {
            let path = self.0.join("source-keel");
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn request(
        scratch: &Scratch,
        source: PathBuf,
        dir: PathBuf,
        handlers: bool,
        path: bool,
    ) -> Request {
        Request {
            layout: scratch.layout(),
            source_binary: source,
            install_dir: dir,
            register_handlers: handlers,
            add_to_path: path,
        }
    }

    #[test]
    fn install_and_uninstall_round_trip() {
        let scratch = Scratch::new("round");
        let layout = scratch.layout();
        fs::create_dir_all(layout.config_home.clone()).unwrap();
        fs::write(
            layout.mimeapps_path(),
            "\
[Default Applications]
text/plain=gedit.desktop
application/x-bittorrent=transmission.desktop
x-scheme-handler/magnet=transmission.desktop

[Added Associations]
text/plain=gedit.desktop;
application/x-bittorrent=transmission.desktop;
",
        )
        .unwrap();

        let dir = layout.default_install_dir();
        let source = scratch.source(b"program-v1");
        let report = install(&request(&scratch, source, dir.clone(), true, true)).unwrap();
        assert!(report.binary.is_file());
        assert_eq!(fs::read(&report.binary).unwrap(), b"program-v1");
        let mode = fs::metadata(&report.binary).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        let desktop = fs::read_to_string(layout.desktop_path()).unwrap();
        assert!(desktop.contains(DESKTOP_MARKER));
        assert!(desktop.contains(&format!("Exec=\"{}\" %U", report.binary.display())));
        assert!(desktop.contains("application/x-bittorrent"));
        assert!(desktop.contains("x-scheme-handler/magnet"));
        let mime = fs::read_to_string(layout.mimeapps_path()).unwrap();
        assert!(mime.contains("text/plain=gedit.desktop\n"));
        assert!(mime.contains("application/x-bittorrent=keel.desktop;"));
        assert!(mime.contains("text/plain=gedit.desktop;"));
        assert!(layout.bin_link().is_symlink());
        assert!(!layout
            .mimeapps_path()
            .with_extension("list.keel-tmp")
            .exists());

        let source = scratch.source(b"program-v2");
        install(&request(&scratch, source, dir.clone(), true, true)).unwrap();
        assert_eq!(fs::read(&report.binary).unwrap(), b"program-v2");
        let manifest = read_manifest(&dir.join(MANIFEST_NAME)).unwrap();
        assert_eq!(
            manifest.previous[0].as_deref(),
            Some("transmission.desktop")
        );

        fs::write(dir.join("notes.txt"), b"keep me").unwrap();
        let removed = uninstall(&layout).unwrap();
        assert!(removed
            .lines
            .iter()
            .any(|line| line.contains("other files")));
        assert_eq!(fs::read(dir.join("notes.txt")).unwrap(), b"keep me");
        assert!(!dir.join(BINARY_NAME).exists());
        assert!(!dir.join(ICON_NAME).exists());
        assert!(!dir.join(MANIFEST_NAME).exists());
        assert!(!layout.desktop_path().exists());
        assert!(!layout.bin_link().exists());
        let mime = fs::read_to_string(layout.mimeapps_path()).unwrap();
        assert!(mime.contains("application/x-bittorrent=transmission.desktop;"));
        assert!(mime.contains("x-scheme-handler/magnet=transmission.desktop;"));
        assert!(mime.contains("text/plain=gedit.desktop"));
        assert!(!mime.contains("keel.desktop"));
        assert!(matches!(
            uninstall(&layout),
            Err(UninstallError::NotInstalled)
        ));
    }

    #[test]
    fn reinstall_can_give_associations_back() {
        let scratch = Scratch::new("handlers");
        let layout = scratch.layout();
        let dir = layout.default_install_dir();
        let source = scratch.source(b"program");
        install(&request(&scratch, source.clone(), dir.clone(), true, false)).unwrap();
        install(&request(&scratch, source, dir, false, false)).unwrap();
        let mime = fs::read_to_string(layout.mimeapps_path()).unwrap_or_default();
        assert!(!mime.contains("keel.desktop"));
        let manifest = read_manifest(&layout.default_install_dir().join(MANIFEST_NAME)).unwrap();
        assert!(!manifest.handlers);
    }

    #[test]
    fn foreign_command_link_is_left_alone() {
        let scratch = Scratch::new("link");
        let layout = scratch.layout();
        fs::create_dir_all(layout.bin_dir()).unwrap();
        let other = scratch.0.join("other");
        fs::write(&other, b"other").unwrap();
        symlink(&other, layout.bin_link()).unwrap();
        let source = scratch.source(b"program");
        let report = install(&request(
            &scratch,
            source,
            layout.default_install_dir(),
            false,
            true,
        ))
        .unwrap();
        assert!(report.lines.iter().any(|line| line.contains("Left")));
        assert_eq!(fs::read_link(layout.bin_link()).unwrap(), other);
        uninstall(&layout).unwrap();
        assert_eq!(fs::read_link(layout.bin_link()).unwrap(), other);
    }

    #[test]
    fn refuses_folders_outside_home_and_links() {
        let scratch = Scratch::new("refuse");
        let layout = scratch.layout();
        assert!(validate_install_dir(&layout, "/tmp").is_err());
        assert!(validate_install_dir(&layout, &layout.home.display().to_string()).is_err());
        assert!(validate_install_dir(&layout, "~/../etc").is_err());
        let linked = scratch.0.join("linked");
        let real = scratch.0.join("real");
        fs::create_dir(&real).unwrap();
        symlink(&real, &linked).unwrap();
        assert!(validate_install_dir(&layout, &linked.display().to_string()).is_err());
        let nested = layout.home.join("apps/keel");
        let resolved = validate_install_dir(&layout, &nested.display().to_string()).unwrap();
        assert_eq!(resolved, nested);
        let tilde = validate_install_dir(&layout, "~/apps/keel").unwrap();
        assert_eq!(tilde, nested);
    }

    #[test]
    fn custom_folder_is_found_from_the_menu_entry() {
        let scratch = Scratch::new("custom");
        let layout = scratch.layout();
        let dir = layout.home.join("apps/keel");
        let source = scratch.source(b"program");
        install(&request(&scratch, source, dir.clone(), true, false)).unwrap();
        assert_eq!(locate(&layout), Some(dir));
        uninstall(&layout).unwrap();
        assert!(locate(&layout).is_none());
    }

    #[test]
    fn installing_over_the_same_file_does_not_truncate_it() {
        let scratch = Scratch::new("same");
        let layout = scratch.layout();
        let dir = layout.default_install_dir();
        fs::create_dir_all(&dir).unwrap();
        let binary = dir.join(BINARY_NAME);
        fs::write(&binary, b"live-program").unwrap();
        install(&request(&scratch, binary.clone(), dir, false, false)).unwrap();
        assert_eq!(fs::read(&binary).unwrap(), b"live-program");
    }

    #[test]
    fn user_who_changed_the_default_keeps_that_choice() {
        let scratch = Scratch::new("choice");
        let layout = scratch.layout();
        let source = scratch.source(b"program");
        install(&request(
            &scratch,
            source,
            layout.default_install_dir(),
            true,
            false,
        ))
        .unwrap();
        let path = layout.mimeapps_path();
        let text = fs::read_to_string(&path).unwrap().replace(
            "x-scheme-handler/magnet=keel.desktop;",
            "x-scheme-handler/magnet=firefox.desktop;",
        );
        fs::write(&path, text).unwrap();
        uninstall(&layout).unwrap();
        let mime = fs::read_to_string(&path).unwrap();
        assert!(mime.contains("x-scheme-handler/magnet=firefox.desktop;"));
        assert!(!mime.contains("keel.desktop"));
    }
}
