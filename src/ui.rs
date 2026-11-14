//! Installer wizard and the library window.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use iced::widget::{
    button, checkbox, column, container, mouse_area, opaque, progress_bar, row, scrollable, stack,
    text, text_editor, text_input, Space,
};
use iced::{window, Background, Border, Color, Element, Fill, Font, Task, Theme};

use crate::access;
use crate::cli::Mode;
use crate::flags;
use crate::inbox;
use crate::install::{self, Report};
use crate::job;
use crate::layout::Layout;
use crate::places::{self, Entry, Place};
use crate::settings;
use crate::source::{self, Opened};
use crate::wire::{self, CreatedTorrent, PeerRow, TorrentRow, TrackerRow};

const REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

#[derive(Debug, Clone)]
pub struct Launch {
    pub mode: Mode,
    pub layout: Layout,
    pub open: Vec<String>,
    pub source_binary: PathBuf,
    /// Held for the life of the library window that owns the inbox.
    pub hold: Option<Arc<inbox::UiLock>>,
}

pub fn run(launch: Launch) -> iced::Result {
    let window_size = match launch.mode {
        Mode::Library | Mode::Engine => iced::Size::new(980.0, 680.0),
        Mode::Wizard | Mode::Uninstall => iced::Size::new(760.0, 640.0),
    };
    let icon = window::icon::from_rgba(crate::icon::rgba(64), 64, 64).ok();

    iced::application(
        {
            let launch = launch.clone();
            move || {
                let mut keel = Keel::new(launch.clone());
                let task = Task::batch([keel.start_opened(), keel.schedule_poll(false)]);
                (keel, task)
            }
        },
        Keel::update,
        Keel::view,
    )
    .title(Keel::title)
    .theme(Keel::theme)
    .settings(iced::Settings {
        id: Some("keel".to_owned()),
        fonts: vec![REGULAR.into(), SEMIBOLD.into()],
        default_font: Font::with_name("Inter"),
        default_text_size: iced::Pixels(17.0),
        ..Default::default()
    })
    .window(window::Settings {
        size: window_size,
        min_size: Some(iced::Size::new(900.0, 560.0)),
        position: window::Position::Centered,
        icon,
        platform_specific: window::settings::PlatformSpecific {
            application_id: "keel".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    })
    .run()
}

#[derive(Clone)]
struct Pending {
    source: String,
    name: String,
    infohash: String,
    error: String,
    submitted: bool,
    accepted: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Selected {
    None,
    Pending(String),
    Torrent(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowseFor {
    Save,
    ShareSource,
    ShareOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Trackers,
    Peers,
}

#[derive(Clone)]
enum MenuTarget {
    Pending(String),
    Torrent(u64),
}

#[derive(Clone)]
struct Browser {
    purpose: BrowseFor,
    path: PathBuf,
    entries: Vec<Entry>,
    places: Vec<Place>,
    picked: Option<usize>,
    error: String,
}

struct Keel {
    screen: Screen,
    layout: Layout,
    source_binary: PathBuf,
    pending: Vec<Pending>,
    torrents: Vec<TorrentRow>,
    download_folder: String,
    selected: Selected,
    add_box: String,
    action_error: String,
    poll_error: String,
    poll_gen: u64,
    hold: Option<Arc<inbox::UiLock>>,
    folder: String,
    register_handlers: bool,
    add_to_path: bool,
    existing: Option<PathBuf>,
    came_from_library: bool,
    launch_error: Option<String>,
    cursor: iced::Point,
    browser: Option<Browser>,
    add_open: bool,
    add_draft: String,
    menu: Option<MenuTarget>,
    delete_id: Option<u64>,
    detail_tab: DetailTab,
    trackers: Vec<TrackerRow>,
    peers: Vec<PeerRow>,
    details_for: Option<u64>,
    details_inflight: bool,
    share_source: String,
    share_name: String,
    share_output: String,
    share_trackers: text_editor::Content,
    share_seed: bool,
    share_result: Option<CreatedTorrent>,
    share_error: String,
    share_busy: bool,
    folder_warning: Option<&'static str>,
    access: Option<access::Problem>,
    access_note: String,
    access_retry: Option<String>,
    access_busy: bool,
    access_hidden: Option<String>,
}

enum Screen {
    Welcome,
    Setup,
    Working {
        title: &'static str,
        detail: &'static str,
    },
    Installed(Report),
    UninstallConfirm,
    Uninstalled(Report),
    Library,
    Share,
    Notice {
        title: String,
        body: String,
        back_to_library: bool,
    },
    Failed {
        title: &'static str,
        message: String,
        back: Back,
    },
}

enum Back {
    Setup,
    UninstallConfirm,
    Close,
}

#[derive(Debug, Clone)]
enum Message {
    Continue,
    Back,
    FolderChanged(String),
    ToggleHandlers(bool),
    TogglePath(bool),
    Install,
    Installed(Result<Report, String>),
    OpenLibrary,
    Close,
    AskUninstall,
    Uninstall,
    Uninstalled(Result<Report, UninstallOutcome>),
    SaveFolder(String),
    AddBox(String),
    AddTyped,
    SelectPending(String),
    SelectTorrent(u64),
    PauseSelected,
    ResumeSelected,
    RemoveSelected,
    Polled {
        generation: u64,
        poll: wire::Snapshot,
    },
    ActionDone {
        source: String,
        result: Result<(), String>,
    },
    EngineDone(Result<(), String>),
    Cursor(iced::Point),
    Browse(BrowseFor),
    BrowseUp,
    BrowsePlace(String),
    BrowseActivate(usize),
    BrowseUse,
    BrowseFile,
    BrowseClose,
    AddOpen,
    AddDraft(String),
    AddStart,
    AddClose,
    ContextPending(String),
    ContextTorrent(u64),
    MenuClose,
    OpenDestination,
    CopyMagnet,
    AskDelete,
    ConfirmDelete,
    CancelDelete,
    ShowDetail(DetailTab),
    DetailsDone {
        id: u64,
        result: Result<wire::Reply, String>,
    },
    ShareOpen,
    ShareBack,
    ShareSource(String),
    ShareName(String),
    ShareOutput(String),
    ShareTrackers(text_editor::Action),
    ShareSeed(bool),
    ShareCreate,
    ShareDone(Result<CreatedTorrent, String>),
    CopyCreated,
    AccessDismiss,
    AccessMount,
    AccessAllow,
    AccessDone(Result<(), String>),
}

#[derive(Debug, Clone)]
enum UninstallOutcome {
    NotInstalled,
    Failed(String),
}

impl Keel {
    fn new(launch: Launch) -> Self {
        let existing = install::locate(&launch.layout);
        let folder = existing
            .clone()
            .unwrap_or_else(|| launch.layout.default_install_dir())
            .display()
            .to_string();
        let screen = match launch.mode {
            Mode::Wizard => Screen::Welcome,
            Mode::Library | Mode::Engine => Screen::Library,
            Mode::Uninstall => {
                if existing.is_some() {
                    Screen::UninstallConfirm
                } else {
                    Screen::Notice {
                        title: "Keel is not installed".into(),
                        body: "There is no Keel installation in this user account.".into(),
                        back_to_library: false,
                    }
                }
            }
        };
        let (download_folder, access) = startup_folder(&launch.layout);
        let folder_warning = folder_warning_for(&download_folder);
        let mut keel = Self {
            screen,
            layout: launch.layout,
            source_binary: launch.source_binary,
            pending: Vec::new(),
            torrents: Vec::new(),
            download_folder,
            selected: Selected::None,
            add_box: String::new(),
            action_error: String::new(),
            poll_error: String::new(),
            poll_gen: 0,
            hold: launch.hold,
            folder,
            register_handlers: true,
            add_to_path: true,
            existing,
            came_from_library: false,
            launch_error: None,
            cursor: iced::Point::ORIGIN,
            browser: None,
            add_open: false,
            add_draft: String::new(),
            menu: None,
            delete_id: None,
            detail_tab: DetailTab::Peers,
            trackers: Vec::new(),
            peers: Vec::new(),
            details_for: None,
            details_inflight: false,
            share_source: String::new(),
            share_name: String::new(),
            share_output: String::new(),
            share_trackers: text_editor::Content::new(),
            share_seed: false,
            share_result: None,
            share_error: String::new(),
            share_busy: false,
            folder_warning,
            access,
            access_note: String::new(),
            access_retry: None,
            access_busy: false,
            access_hidden: None,
        };
        for item in source::normalize_args(launch.open) {
            keel.push_pending(item);
        }
        keel
    }

    fn schedule_poll(&mut self, wait: bool) -> Task<Message> {
        if !matches!(self.screen, Screen::Library) {
            return Task::none();
        }
        self.poll_gen = self.poll_gen.wrapping_add(1);
        let generation = self.poll_gen;
        let layout = self.layout.clone();
        let binary = self.source_binary.clone();
        let read_inbox = self.hold.is_some();
        Task::perform(
            job::on_thread(move || wire::poll(&layout, &binary, wait, read_inbox)),
            move |poll| Message::Polled { generation, poll },
        )
    }

    fn push_pending(&mut self, raw: String) -> bool {
        let Opened {
            source,
            name,
            infohash,
        } = source::describe(&raw);
        if source.is_empty()
            || self.pending.iter().any(|item| item.source == source)
            || self
                .torrents
                .iter()
                .any(|row| same_transfer(row, &name, &infohash))
        {
            return false;
        }
        if self.selected == Selected::None {
            self.selected = Selected::Pending(source.clone());
        }
        self.pending.push(Pending {
            source,
            name,
            infohash,
            error: String::new(),
            submitted: false,
            accepted: false,
        });
        true
    }

    /// Start every opened torrent with the save folder. The row is already visible.
    fn start_opened(&mut self) -> Task<Message> {
        if !matches!(self.screen, Screen::Library) {
            return Task::none();
        }
        let folder = self.download_folder.clone();
        let sources: Vec<String> = self
            .pending
            .iter()
            .filter(|item| !item.submitted && item.error.is_empty())
            .map(|item| item.source.clone())
            .collect();
        if sources.is_empty() {
            return Task::none();
        }
        for item in &mut self.pending {
            if sources.contains(&item.source) {
                item.submitted = true;
                item.error.clear();
            }
        }
        let layout = self.layout.clone();
        let binary = self.source_binary.clone();
        Task::batch(sources.into_iter().map(move |source| {
            let layout = layout.clone();
            let binary = binary.clone();
            let folder = folder.clone();
            let reported = source.clone();
            Task::perform(
                job::on_thread(move || {
                    let folder = source::prepare_download_dir(&layout, &folder)?;
                    wire::add(&layout, &binary, &source, &folder.display().to_string())
                }),
                move |result| Message::ActionDone {
                    source: reported,
                    result,
                },
            )
        }))
    }

    fn add_source(&mut self, raw: String, clear_box: bool) -> Task<Message> {
        let text = raw.trim().to_owned();
        if text.is_empty() {
            return Task::none();
        }
        if clear_box {
            self.add_box.clear();
            self.add_draft.clear();
        }
        self.push_pending(text);
        self.start_opened()
    }

    fn close_overlays(&mut self) {
        self.browser = None;
        self.add_open = false;
        self.menu = None;
        self.delete_id = None;
    }

    fn request_details(&mut self) -> Task<Message> {
        let Selected::Torrent(id) = self.selected else {
            return Task::none();
        };
        if self.details_inflight || !matches!(self.screen, Screen::Library) {
            return Task::none();
        }
        self.details_inflight = true;
        let layout = self.layout.clone();
        let binary = self.source_binary.clone();
        Task::perform(
            job::on_thread(move || wire::details(&layout, &binary, id)),
            move |result| Message::DetailsDone { id, result },
        )
    }

    fn open_browser(&mut self, purpose: BrowseFor) {
        self.menu = None;
        self.add_open = false;
        self.delete_id = None;
        let path = self.browser_start(purpose);
        let home = self.layout.home.clone();
        let (path, entries, error) = match places::list_dir(&path) {
            Ok(entries) => (path, entries, String::new()),
            Err(message) => match places::list_dir(&home) {
                Ok(entries) => {
                    let shown = self.report(&path, &message, None);
                    (home, entries, shown)
                }
                Err(home_message) => {
                    let shown = self.report(&home, &home_message, None);
                    (home, Vec::new(), shown)
                }
            },
        };
        self.browser = Some(Browser {
            purpose,
            path,
            entries,
            places: places::places(&self.layout),
            picked: None,
            error,
        });
    }

    fn browser_start(&self, purpose: BrowseFor) -> PathBuf {
        let typed = match purpose {
            BrowseFor::Save => self.download_folder.trim(),
            BrowseFor::ShareSource => self.share_source.trim(),
            BrowseFor::ShareOutput => self.share_output.trim(),
        };
        let path = PathBuf::from(typed);
        if path.is_dir() {
            path
        } else if path.parent().is_some_and(|parent| parent.is_dir()) {
            path.parent().unwrap_or(&self.layout.home).to_path_buf()
        } else {
            self.layout.home.clone()
        }
    }

    fn browser_open_path(&mut self, path: PathBuf) {
        match places::list_dir(&path) {
            Ok(entries) => {
                if let Some(browser) = self.browser.as_mut() {
                    browser.path = path;
                    browser.entries = entries;
                    browser.picked = None;
                    browser.error.clear();
                }
            }
            Err(message) => {
                let shown = self.report(&path, &message, None);
                if let Some(browser) = self.browser.as_mut() {
                    browser.error = shown;
                }
            }
        }
    }

    fn browser_up(&mut self) {
        let parent = self.browser.as_ref().and_then(|browser| {
            browser
                .path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
        });
        let Some(parent) = parent else {
            return;
        };
        self.browser_open_path(parent);
    }

    fn browser_activate(&mut self, index: usize) {
        let action = self.browser.as_ref().and_then(|browser| {
            browser
                .entries
                .get(index)
                .map(|entry| (entry.is_dir, entry.path.clone()))
        });
        let Some((is_dir, path)) = action else {
            return;
        };
        if is_dir {
            self.browser_open_path(path);
        } else if let Some(browser) = self.browser.as_mut() {
            browser.picked = Some(index);
        }
    }

    fn browser_use(&mut self, file: bool) {
        let Some(browser) = self.browser.clone() else {
            return;
        };
        if file {
            let Some(index) = browser.picked else {
                if let Some(slot) = self.browser.as_mut() {
                    slot.error = "Choose a file first.".into();
                }
                return;
            };
            let Some(entry) = browser.entries.get(index) else {
                return;
            };
            if entry.is_dir {
                if let Some(slot) = self.browser.as_mut() {
                    slot.error = "That is a folder. Use this folder, or choose a file.".into();
                }
                return;
            }
            if let Err(message) = source::reject_links(&entry.path) {
                let shown = self.report(&entry.path, &message, None);
                if let Some(slot) = self.browser.as_mut() {
                    slot.error = shown;
                }
                return;
            }
            self.share_source = entry.path.display().to_string();
            if self.share_name.trim().is_empty() {
                self.share_name = entry.name.clone();
            }
            if self.share_output.trim().is_empty() {
                if let Some(parent) = entry.path.parent() {
                    self.share_output = parent.display().to_string();
                }
            }
            self.browser = None;
            return;
        }
        match browser.purpose {
            BrowseFor::ShareSource => {
                if let Err(message) = source::reject_links(&browser.path) {
                    let shown = self.report(&browser.path, &message, None);
                    if let Some(slot) = self.browser.as_mut() {
                        slot.error = shown;
                    }
                    return;
                }
                self.share_source = browser.path.display().to_string();
                if self.share_name.trim().is_empty() {
                    if let Some(name) = browser.path.file_name().and_then(|name| name.to_str()) {
                        self.share_name = name.to_owned();
                    }
                }
                if self.share_output.trim().is_empty() {
                    self.share_output = browser.path.display().to_string();
                }
                self.browser = None;
            }
            BrowseFor::ShareOutput => match source::prepare_download_dir(
                &self.layout,
                &browser.path.display().to_string(),
            ) {
                Ok(path) => {
                    self.share_output = path.display().to_string();
                    self.browser = None;
                }
                Err(message) => {
                    let shown = self.report(&browser.path, &message, None);
                    if let Some(slot) = self.browser.as_mut() {
                        slot.error = shown;
                    }
                }
            },
            BrowseFor::Save => {
                match source::prepare_download_dir(
                    &self.layout,
                    &browser.path.display().to_string(),
                ) {
                    Ok(path) => {
                        self.download_folder = path.display().to_string();
                        self.folder_warning = folder_warning_for(&self.download_folder);
                        if let Err(message) = settings::save_download_dir(&self.layout, &path) {
                            self.action_error = self.report(&path, &message, None);
                        }
                        self.browser = None;
                    }
                    Err(message) => {
                        let shown = self.report(&browser.path, &message, None);
                        if let Some(slot) = self.browser.as_mut() {
                            slot.error = shown;
                        }
                    }
                }
            }
        }
    }

    fn acted_torrent(&self) -> Option<u64> {
        match &self.menu {
            Some(MenuTarget::Torrent(id)) => Some(*id),
            _ => match self.selected {
                Selected::Torrent(id) => Some(id),
                _ => None,
            },
        }
    }

    fn destination_folder(&self) -> Option<String> {
        match &self.selected {
            Selected::Torrent(id) => self
                .torrents
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.folder.clone())
                .filter(|folder| !folder.is_empty()),
            Selected::Pending(_) => Some(self.download_folder.trim().to_owned()),
            Selected::None => None,
        }
    }

    fn magnet_text(&self) -> Option<String> {
        match &self.selected {
            Selected::Pending(source) => self
                .pending
                .iter()
                .find(|item| &item.source == source)
                .and_then(|item| {
                    if item.source.starts_with("magnet:") {
                        Some(item.source.clone())
                    } else if !item.infohash.is_empty() {
                        Some(source::magnet_uri(&item.infohash, &item.name, &[]))
                    } else {
                        None
                    }
                }),
            Selected::Torrent(id) => {
                self.torrents
                    .iter()
                    .find(|row| row.id == *id)
                    .and_then(|row| {
                        if row.infohash.is_empty() {
                            return None;
                        }
                        let trackers = if self.details_for == Some(*id) {
                            self.trackers
                                .iter()
                                .map(|tracker| tracker.url.clone())
                                .collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
                        Some(source::magnet_uri(&row.infohash, &row.name, &trackers))
                    })
            }
            Selected::None => None,
        }
    }

    fn engine_call<F>(&self, work: F) -> Task<Message>
    where
        F: FnOnce(Layout, PathBuf) -> Result<(), String> + Send + 'static,
    {
        let layout = self.layout.clone();
        let binary = self.source_binary.clone();
        Task::perform(
            job::on_thread(move || work(layout, binary)),
            Message::EngineDone,
        )
    }

    fn report(&mut self, hint: &Path, message: &str, retry: Option<String>) -> String {
        let shown = access::explain(hint, message);
        self.raise_access(&shown, retry, true);
        shown
    }

    fn raise_access(&mut self, shown: &str, retry: Option<String>, force: bool) {
        let Some(problem) = access::problem_from_sentence(shown) else {
            return;
        };
        if self.access.is_some() && !force {
            return;
        }
        let sentence = problem.sentence();
        if !force && self.access_hidden.as_deref() == Some(sentence.as_str()) {
            return;
        }
        self.access = Some(problem);
        self.access_note.clear();
        self.access_busy = false;
        if retry.is_some() {
            self.access_retry = retry;
        }
        if force {
            self.access_hidden = None;
        }
    }

    #[cfg(test)]
    fn show_problem(&mut self, hint: &Path, message: &str) {
        self.action_error = self.report(hint, message, None);
    }

    fn title(&self) -> String {
        match self.screen {
            Screen::Library | Screen::Share => "Keel".into(),
            Screen::UninstallConfirm | Screen::Uninstalled(_) => "Uninstall Keel".into(),
            _ => "Install Keel".into(),
        }
    }

    fn theme(&self) -> Theme {
        keel_theme()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Continue => {
                self.screen = Screen::Setup;
                Task::none()
            }
            Message::Back => {
                self.screen = match &self.screen {
                    Screen::Failed {
                        back: Back::Setup, ..
                    } => Screen::Setup,
                    Screen::Failed {
                        back: Back::UninstallConfirm,
                        ..
                    } => Screen::UninstallConfirm,
                    Screen::Setup => Screen::Welcome,
                    Screen::UninstallConfirm
                    | Screen::Notice {
                        back_to_library: true,
                        ..
                    } => Screen::Library,
                    _ => Screen::Welcome,
                };
                if matches!(self.screen, Screen::Library) {
                    self.schedule_poll(false)
                } else {
                    Task::none()
                }
            }
            Message::FolderChanged(folder) => {
                self.folder = folder;
                Task::none()
            }
            Message::ToggleHandlers(value) => {
                self.register_handlers = value;
                Task::none()
            }
            Message::TogglePath(value) => {
                self.add_to_path = value;
                Task::none()
            }
            Message::Install => match install::validate_install_dir(&self.layout, &self.folder) {
                Ok(install_dir) => {
                    self.folder = install_dir.display().to_string();
                    let request = install::Request {
                        layout: self.layout.clone(),
                        source_binary: self.source_binary.clone(),
                        install_dir,
                        register_handlers: self.register_handlers,
                        add_to_path: self.add_to_path,
                    };
                    self.screen = Screen::Working {
                        title: "Installing Keel",
                        detail: "Copying the program and registering it.",
                    };
                    Task::perform(
                        job::on_thread(move || install::install(&request)),
                        Message::Installed,
                    )
                }
                Err(message) => {
                    let shown = access::explain(Path::new(&self.folder), &message);
                    self.raise_access(&shown, None, true);
                    self.screen = Screen::Failed {
                        title: "Choose a different folder",
                        message: shown,
                        back: Back::Setup,
                    };
                    Task::none()
                }
            },
            Message::Installed(Ok(report)) => {
                self.existing = Some(
                    report
                        .binary
                        .parent()
                        .unwrap_or(&report.binary)
                        .to_path_buf(),
                );
                self.launch_error = None;
                self.screen = Screen::Installed(report);
                Task::none()
            }
            Message::Installed(Err(message)) => {
                let shown = access::explain(Path::new(&self.folder), &message);
                self.raise_access(&shown, None, true);
                self.screen = Screen::Failed {
                    title: "Keel was not installed",
                    message: shown,
                    back: Back::Setup,
                };
                Task::none()
            }
            Message::OpenLibrary => {
                let binary = match &self.screen {
                    Screen::Installed(report) => report.binary.clone(),
                    _ => self.layout.default_install_dir().join("keel"),
                };
                match std::process::Command::new(&binary).arg("--app").spawn() {
                    Ok(_) => iced::exit(),
                    Err(err) => {
                        self.launch_error = Some(self.report(
                            &binary,
                            &format!("Could not open Keel: {err}"),
                            None,
                        ));
                        Task::none()
                    }
                }
            }
            Message::Close => iced::exit(),
            Message::AskUninstall => {
                self.close_overlays();
                if install::locate(&self.layout).is_none() {
                    self.screen = Screen::Notice {
                        title: "Keel is not installed".into(),
                        body: "There is no Keel installation in this user account.".into(),
                        back_to_library: true,
                    };
                } else {
                    self.came_from_library = true;
                    self.screen = Screen::UninstallConfirm;
                }
                Task::none()
            }
            Message::Uninstall => {
                let layout = self.layout.clone();
                self.screen = Screen::Working {
                    title: "Removing Keel",
                    detail:
                        "Taking the program off this account. Downloaded files stay where they are.",
                };
                Task::perform(
                    job::on_thread(move || match install::uninstall(&layout) {
                        Ok(report) => Ok(report),
                        Err(install::UninstallError::NotInstalled) => {
                            Err(UninstallOutcome::NotInstalled)
                        }
                        Err(install::UninstallError::Failed(message)) => {
                            Err(UninstallOutcome::Failed(message))
                        }
                    }),
                    Message::Uninstalled,
                )
            }
            Message::Uninstalled(Ok(report)) => {
                self.existing = None;
                self.screen = Screen::Uninstalled(report);
                Task::none()
            }
            Message::Uninstalled(Err(UninstallOutcome::NotInstalled)) => {
                self.screen = Screen::Notice {
                    title: "Keel is not installed".into(),
                    body: "There is no Keel installation in this user account.".into(),
                    back_to_library: self.came_from_library,
                };
                Task::none()
            }
            Message::Uninstalled(Err(UninstallOutcome::Failed(message))) => {
                let shown = access::explain(Path::new(&self.folder), &message);
                self.raise_access(&shown, None, true);
                self.screen = Screen::Failed {
                    title: "Keel was not removed",
                    message: shown,
                    back: if self.came_from_library {
                        Back::UninstallConfirm
                    } else {
                        Back::Close
                    },
                };
                Task::none()
            }
            Message::SaveFolder(value) => {
                self.download_folder = value;
                self.folder_warning = folder_warning_for(&self.download_folder);
                self.action_error.clear();
                Task::none()
            }
            Message::AddBox(value) => {
                self.add_box = value;
                self.action_error.clear();
                Task::none()
            }
            Message::AddTyped => self.add_source(self.add_box.clone(), true),
            Message::SelectPending(source) => {
                self.selected = Selected::Pending(source);
                self.menu = None;
                Task::none()
            }
            Message::SelectTorrent(id) => {
                self.selected = Selected::Torrent(id);
                self.menu = None;
                Task::none()
            }
            Message::PauseSelected => {
                let id = self.acted_torrent();
                self.menu = None;
                match id {
                    Some(id) => {
                        self.engine_call(move |layout, binary| wire::pause(&layout, &binary, id))
                    }
                    None => Task::none(),
                }
            }
            Message::ResumeSelected => {
                let menu = self.menu.clone();
                self.menu = None;
                match menu {
                    Some(MenuTarget::Pending(source)) => self.add_source(source, false),
                    Some(MenuTarget::Torrent(id)) => {
                        self.engine_call(move |layout, binary| wire::resume(&layout, &binary, id))
                    }
                    None => match self.selected {
                        Selected::Torrent(id) => self
                            .engine_call(move |layout, binary| wire::resume(&layout, &binary, id)),
                        _ => Task::none(),
                    },
                }
            }
            Message::RemoveSelected => {
                let menu = self.menu.clone();
                self.menu = None;
                match menu {
                    Some(MenuTarget::Torrent(id)) => {
                        if self.selected == Selected::Torrent(id) {
                            self.selected = Selected::None;
                        }
                        self.engine_call(move |layout, binary| wire::remove(&layout, &binary, id))
                    }
                    Some(MenuTarget::Pending(source)) => {
                        self.pending.retain(|item| item.source != source);
                        self.selected = Selected::None;
                        Task::none()
                    }
                    None => match &self.selected {
                        Selected::Torrent(id) => {
                            let id = *id;
                            self.selected = Selected::None;
                            self.engine_call(move |layout, binary| {
                                wire::remove(&layout, &binary, id)
                            })
                        }
                        Selected::Pending(source) => {
                            let source = source.clone();
                            self.pending.retain(|item| item.source != source);
                            self.selected = Selected::None;
                            Task::none()
                        }
                        Selected::None => Task::none(),
                    },
                }
            }
            Message::Polled { generation, poll } => {
                if generation != self.poll_gen {
                    return Task::none();
                }
                let mut start = Task::none();
                if poll.error.is_empty() {
                    self.torrents = poll.torrents;
                    for row in &mut self.torrents {
                        if !row.error.is_empty() {
                            let folder = row.folder.clone();
                            row.error = access::explain(Path::new(&folder), &row.error);
                        }
                    }
                    let row_problem = self
                        .torrents
                        .iter()
                        .find(|row| access::problem_from_sentence(&row.error).is_some())
                        .map(|row| row.error.clone());
                    if let Some(error) = row_problem {
                        self.raise_access(&error, None, false);
                    }
                    self.poll_error.clear();
                    self.pending.retain(|item| {
                        !self
                            .torrents
                            .iter()
                            .any(|row| same_transfer(row, &item.name, &item.infohash))
                    });
                } else {
                    let shown = access::explain(&self.layout.socket_path(), &poll.error);
                    self.poll_error = shown.clone();
                    self.raise_access(&shown, None, false);
                }
                if !matches!(self.selected, Selected::None) {
                    let still_there = match &self.selected {
                        Selected::Torrent(id) => self.torrents.iter().any(|row| row.id == *id),
                        Selected::Pending(source) => {
                            self.pending.iter().any(|item| &item.source == source)
                        }
                        Selected::None => false,
                    };
                    if !still_there {
                        self.selected = Selected::None;
                    }
                }
                for item in poll.inbox {
                    self.push_pending(item);
                }
                if matches!(self.screen, Screen::Library) {
                    start = self.start_opened();
                    Task::batch([start, self.request_details(), self.schedule_poll(true)])
                } else {
                    start
                }
            }
            Message::ActionDone { source, result } => {
                if let Some(item) = self.pending.iter_mut().find(|item| item.source == source) {
                    match &result {
                        Ok(()) => {
                            item.accepted = true;
                            item.error.clear();
                        }
                        Err(_) => {
                            item.submitted = false;
                        }
                    }
                }
                match result {
                    Ok(()) => {
                        self.action_error.clear();
                        self.access = None;
                        self.access_retry = None;
                        self.access_note.clear();
                        if let Ok(path) =
                            source::prepare_download_dir(&self.layout, &self.download_folder)
                        {
                            self.download_folder = path.display().to_string();
                            self.folder_warning = folder_warning_for(&self.download_folder);
                            let _ = settings::save_download_dir(&self.layout, &path);
                        }
                    }
                    Err(message) => {
                        let hint = PathBuf::from(self.download_folder.trim());
                        let shown = self.report(&hint, &message, Some(source.clone()));
                        if let Some(item) =
                            self.pending.iter_mut().find(|item| item.source == source)
                        {
                            item.error = shown.clone();
                        }
                        self.action_error = shown;
                    }
                }
                Task::none()
            }
            Message::EngineDone(Ok(())) => {
                self.action_error.clear();
                Task::none()
            }
            Message::EngineDone(Err(message)) => {
                let hint = self
                    .destination_folder()
                    .unwrap_or_else(|| self.download_folder.clone());
                self.action_error = self.report(Path::new(&hint), &message, None);
                Task::none()
            }
            Message::Cursor(point) => {
                self.cursor = point;
                Task::none()
            }
            Message::Browse(purpose) => {
                self.open_browser(purpose);
                Task::none()
            }
            Message::BrowseUp => {
                self.browser_up();
                Task::none()
            }
            Message::BrowsePlace(path) => {
                self.browser_open_path(PathBuf::from(path));
                Task::none()
            }
            Message::BrowseActivate(index) => {
                self.browser_activate(index);
                Task::none()
            }
            Message::BrowseUse => {
                self.browser_use(false);
                Task::none()
            }
            Message::BrowseFile => {
                self.browser_use(true);
                Task::none()
            }
            Message::BrowseClose => {
                self.browser = None;
                Task::none()
            }
            Message::AddOpen => {
                self.add_draft = self.add_box.clone();
                self.add_open = true;
                self.menu = None;
                self.browser = None;
                self.delete_id = None;
                Task::none()
            }
            Message::AddDraft(value) => {
                self.add_draft = value;
                Task::none()
            }
            Message::AddStart => {
                let draft = self.add_draft.clone();
                self.add_open = false;
                self.add_source(draft, true)
            }
            Message::AddClose => {
                self.add_open = false;
                Task::none()
            }
            Message::ContextPending(source) => {
                self.selected = Selected::Pending(source.clone());
                self.menu = Some(MenuTarget::Pending(source));
                self.add_open = false;
                self.browser = None;
                self.delete_id = None;
                Task::none()
            }
            Message::ContextTorrent(id) => {
                self.selected = Selected::Torrent(id);
                self.menu = Some(MenuTarget::Torrent(id));
                self.add_open = false;
                self.browser = None;
                self.delete_id = None;
                Task::none()
            }
            Message::MenuClose => {
                self.menu = None;
                Task::none()
            }
            Message::OpenDestination => {
                self.menu = None;
                let Some(folder) = self.destination_folder() else {
                    self.action_error = "Choose a torrent first.".into();
                    return Task::none();
                };
                Task::perform(
                    job::on_thread(move || open_folder(&folder)),
                    Message::EngineDone,
                )
            }
            Message::CopyMagnet => {
                self.menu = None;
                match self.magnet_text() {
                    Some(magnet) => {
                        self.action_error.clear();
                        iced::clipboard::write(magnet)
                    }
                    None => {
                        self.action_error = "This item has no magnet link yet.".into();
                        Task::none()
                    }
                }
            }
            Message::AskDelete => {
                self.menu = None;
                self.delete_id = match self.selected {
                    Selected::Torrent(id) => Some(id),
                    _ => None,
                };
                if self.delete_id.is_none() {
                    self.action_error = "Choose a torrent whose files Keel already knows.".into();
                }
                Task::none()
            }
            Message::CancelDelete => {
                self.delete_id = None;
                Task::none()
            }
            Message::ConfirmDelete => {
                let Some(id) = self.delete_id else {
                    return Task::none();
                };
                self.delete_id = None;
                if self.selected == Selected::Torrent(id) {
                    self.selected = Selected::None;
                }
                self.engine_call(move |layout, binary| wire::delete_files(&layout, &binary, id))
            }
            Message::ShowDetail(tab) => {
                self.detail_tab = tab;
                Task::none()
            }
            Message::DetailsDone { id, result } => {
                self.details_inflight = false;
                if self.selected != Selected::Torrent(id) {
                    return Task::none();
                }
                match result {
                    Ok(reply) => {
                        self.details_for = Some(id);
                        self.trackers = reply.trackers;
                        self.peers = reply.peers;
                    }
                    Err(message) => {
                        if self.action_error.is_empty() {
                            let hint = self.download_folder.clone();
                            self.action_error = self.report(Path::new(&hint), &message, None);
                        }
                    }
                }
                Task::none()
            }
            Message::ShareOpen => {
                self.close_overlays();
                if self.share_output.trim().is_empty() {
                    self.share_output = self.download_folder.clone();
                }
                self.screen = Screen::Share;
                Task::none()
            }
            Message::ShareBack => {
                self.screen = Screen::Library;
                self.schedule_poll(false)
            }
            Message::ShareSource(value) => {
                self.share_source = value;
                self.share_error.clear();
                Task::none()
            }
            Message::ShareName(value) => {
                self.share_name = value;
                Task::none()
            }
            Message::ShareOutput(value) => {
                self.share_output = value;
                self.share_error.clear();
                Task::none()
            }
            Message::ShareTrackers(action) => {
                self.share_trackers.perform(action);
                Task::none()
            }
            Message::ShareSeed(value) => {
                self.share_seed = value;
                Task::none()
            }
            Message::ShareCreate => {
                if self.share_busy {
                    return Task::none();
                }
                self.share_busy = true;
                self.share_error.clear();
                self.share_result = None;
                let layout = self.layout.clone();
                let binary = self.source_binary.clone();
                let source = self.share_source.clone();
                let name = self.share_name.clone();
                let output = self.share_output.clone();
                let trackers = tracker_lines(&self.share_trackers.text());
                let seed = self.share_seed;
                Task::perform(
                    job::on_thread(move || {
                        wire::create(
                            &layout,
                            &binary,
                            source.trim(),
                            name.trim(),
                            &trackers,
                            output.trim(),
                            seed,
                        )
                    }),
                    Message::ShareDone,
                )
            }
            Message::ShareDone(result) => {
                self.share_busy = false;
                match result {
                    Ok(created) => {
                        self.share_error.clear();
                        self.share_result = Some(created);
                    }
                    Err(message) => {
                        let hint = if self.share_output.trim().is_empty() {
                            self.share_source.clone()
                        } else {
                            self.share_output.clone()
                        };
                        let shown = access::explain(Path::new(&hint), &message);
                        self.raise_access(&shown, None, true);
                        self.share_error = if message.contains("seeding did not start") {
                            let scrubbed = access::plain(&message);
                            if scrubbed.contains("seeding did not start") {
                                scrubbed
                            } else {
                                "The torrent file was written, but seeding did not start.".into()
                            }
                        } else {
                            shown
                        };
                    }
                }
                Task::none()
            }
            Message::CopyCreated => {
                let Some(created) = &self.share_result else {
                    return Task::none();
                };
                iced::clipboard::write(created.magnet.clone())
            }
            Message::AccessDismiss => {
                if let Some(problem) = &self.access {
                    self.access_hidden = Some(problem.sentence());
                }
                self.access = None;
                self.access_note.clear();
                self.access_retry = None;
                self.access_busy = false;
                Task::none()
            }
            Message::AccessMount => {
                let Some(access::Problem::Mount { label, .. }) = &self.access else {
                    return Task::none();
                };
                if self.access_busy {
                    return Task::none();
                }
                self.access_busy = true;
                let label = label.clone();
                Task::perform(
                    job::on_thread(move || access::mount_drive(&label)),
                    Message::AccessDone,
                )
            }
            Message::AccessAllow => {
                let Some(access::Problem::Write { path }) = &self.access else {
                    return Task::none();
                };
                if self.access_busy {
                    return Task::none();
                }
                self.access_busy = true;
                let path = path.clone();
                Task::perform(
                    job::on_thread(move || access::allow_write(&path)),
                    Message::AccessDone,
                )
            }
            Message::AccessDone(result) => {
                self.access_busy = false;
                match result {
                    Ok(()) => {
                        let retry = self.access_retry.take();
                        self.access = None;
                        self.access_note.clear();
                        self.access_hidden = None;
                        self.action_error.clear();
                        self.folder_warning = folder_warning_for(&self.download_folder);
                        if let Some(source) = retry {
                            if let Some(item) =
                                self.pending.iter_mut().find(|item| item.source == source)
                            {
                                item.submitted = false;
                                item.error.clear();
                            }
                            self.start_opened()
                        } else {
                            Task::none()
                        }
                    }
                    Err(message) => {
                        self.access_note = access::plain(&message);
                        Task::none()
                    }
                }
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let page = match &self.screen {
            Screen::Welcome => self.welcome(),
            Screen::Setup => self.setup(),
            Screen::Working { title, detail } => self.working(title, detail),
            Screen::Installed(report) => self.installed(report),
            Screen::UninstallConfirm => self.uninstall_confirm(),
            Screen::Uninstalled(report) => self.uninstalled(report),
            Screen::Library => self.library(),
            Screen::Share => self.share(),
            Screen::Notice {
                title,
                body,
                back_to_library,
            } => self.notice(title, body, *back_to_library),
            Screen::Failed {
                title,
                message,
                back,
            } => self.failed(title, message, back),
        };
        let pad = if matches!(self.screen, Screen::Library | Screen::Share) {
            16.0
        } else {
            36.0
        };
        let padded = container(page).width(Fill).height(Fill).padding(pad);
        let library = matches!(self.screen, Screen::Library | Screen::Share);
        if !library && self.access.is_none() {
            return padded.into();
        }
        let base: Element<'_, Message> = if library {
            mouse_area(padded).on_move(Message::Cursor).into()
        } else {
            padded.into()
        };
        let mut layers: Vec<Element<'_, Message>> = vec![base];
        if library {
            if let Some(extra) = self.card_overlay() {
                layers.push(extra);
            }
        }
        if self.access.is_some() {
            layers.push(self.access_overlay());
        }
        stack(layers).width(Fill).height(Fill).into()
    }

    fn welcome(&self) -> Element<'_, Message> {
        let mut body = column![
            kicker("Step 1 of 2"),
            heading("Install Keel"),
            body_text("Keel installs into your user account. It does not use a package manager, and it does not ask for an administrator password."),
            body_text("It adds a menu entry, and it can open .torrent files and magnet links."),
            body_text("The library downloads a torrent into a folder you choose."),
        ]
        .spacing(14);

        if let Some(dir) = &self.existing {
            body = body.push(body_text(format!(
                "Keel is already installed in {}. Continuing replaces the program there.",
                dir.display()
            )));
        }

        wizard(
            body.into(),
            row![
                Space::new().width(Fill),
                primary("Continue", Message::Continue)
            ]
            .into(),
        )
    }

    fn setup(&self) -> Element<'_, Message> {
        let hint: Element<'_, Message> =
            match install::validate_install_dir(&self.layout, &self.folder) {
                Ok(path) => muted(format!("The program will be copied to {}.", path.display())),
                Err(message) => text(message).size(15).style(danger_text).into(),
            };
        let path_help = if crate::layout::dir_on_path(
            &self.layout.bin_dir(),
            std::env::var("PATH").ok().as_deref(),
        ) {
            format!("Creates a link at {}.", self.layout.bin_link().display())
        } else {
            format!(
                "Creates a link at {}. That folder is not on PATH in this session. The menu entry still works.",
                self.layout.bin_link().display()
            )
        };

        let form = column![
            kicker("Step 2 of 2"),
            heading("Where should Keel live?"),
            text_input(
                &self.layout.default_install_dir().display().to_string(),
                &self.folder,
            )
            .on_input(Message::FolderChanged)
            .padding(12)
            .size(16)
            .width(Fill),
            hint,
            check_row(
                "Open .torrent files and magnet links with Keel",
                "This account's current choice is remembered and put back if you uninstall.",
                self.register_handlers,
                Message::ToggleHandlers,
            ),
            check_row(
                "Put the keel command on PATH",
                &path_help,
                self.add_to_path,
                Message::TogglePath,
            ),
        ]
        .spacing(14);

        wizard(
            form.into(),
            row![
                quiet("Back", Message::Back),
                Space::new().width(Fill),
                primary("Install", Message::Install),
            ]
            .align_y(iced::Center)
            .into(),
        )
    }

    fn working(&self, title: &'static str, detail: &'static str) -> Element<'_, Message> {
        wizard(
            column![heading(title), body_text(detail)]
                .spacing(14)
                .into(),
            Space::new().width(Fill).into(),
        )
    }

    fn installed(&self, report: &Report) -> Element<'_, Message> {
        let mut lines = column![heading("Keel is installed")].spacing(10);
        for line in &report.lines {
            lines = lines.push(body_text(line));
        }
        if let Some(error) = &self.launch_error {
            lines = lines.push(text(access::plain(error)).size(16).style(danger_text));
        }
        wizard(
            lines.into(),
            row![
                quiet("Close", Message::Close),
                Space::new().width(Fill),
                primary("Open Keel", Message::OpenLibrary),
            ]
            .align_y(iced::Center)
            .into(),
        )
    }

    fn uninstall_confirm(&self) -> Element<'_, Message> {
        let back: Element<'_, Message> = if self.came_from_library {
            quiet("Back", Message::Back)
        } else {
            quiet("Close", Message::Close)
        };
        wizard(
            column![
                heading("Remove Keel?"),
                body_text("This removes the program, the menu entry, and the file associations Keel registered."),
                body_text("Downloaded files are left where they are."),
            ]
            .spacing(14)
            .into(),
            row![back, Space::new().width(Fill), danger("Remove Keel", Message::Uninstall)]
                .align_y(iced::Center)
                .into(),
        )
    }

    fn uninstalled(&self, report: &Report) -> Element<'_, Message> {
        let mut lines = column![heading("Keel was removed")].spacing(10);
        for line in &report.lines {
            lines = lines.push(body_text(line));
        }
        wizard(
            lines.into(),
            row![Space::new().width(Fill), primary("Close", Message::Close)].into(),
        )
    }

    fn notice(&self, title: &str, body: &str, back_to_library: bool) -> Element<'_, Message> {
        let actions: Element<'_, Message> = if back_to_library {
            row![
                quiet("Back", Message::Back),
                Space::new().width(Fill),
                primary("Close", Message::Close),
            ]
            .align_y(iced::Center)
            .into()
        } else {
            row![Space::new().width(Fill), primary("Close", Message::Close)].into()
        };
        wizard(
            column![heading(title), body_text(body)].spacing(14).into(),
            actions,
        )
    }

    fn failed(&self, title: &str, message: &str, back: &Back) -> Element<'_, Message> {
        let back_button: Element<'_, Message> = match back {
            Back::Setup | Back::UninstallConfirm => quiet("Back", Message::Back),
            Back::Close => quiet("Close", Message::Close),
        };
        wizard(
            column![
                heading(title),
                text(access::plain(message)).size(17).style(danger_text)
            ]
            .spacing(14)
            .into(),
            row![back_button, Space::new().width(Fill)].into(),
        )
    }

    fn library(&self) -> Element<'_, Message> {
        let pause = match self.selected {
            Selected::Torrent(id) => {
                let paused = self
                    .torrents
                    .iter()
                    .find(|row| row.id == id)
                    .is_some_and(|row| row.state == "paused" || row.finished);
                if paused {
                    command("Resume", Some(Message::ResumeSelected))
                } else {
                    command("Pause", Some(Message::PauseSelected))
                }
            }
            _ => command("Pause", None),
        };
        let remove = match self.selected {
            Selected::None => command("Remove", None),
            _ => command("Remove", Some(Message::RemoveSelected)),
        };
        let raw_note = if !self.action_error.is_empty() {
            self.action_error.clone()
        } else {
            self.poll_error.clone()
        };
        let note = if raw_note.is_empty() {
            String::new()
        } else {
            access::explain(Path::new(self.download_folder.trim()), &raw_note)
        };
        let down: u64 = self.torrents.iter().map(|row| row.down_bps).sum();
        let up: u64 = self.torrents.iter().map(|row| row.up_bps).sum();
        let count = self.torrents.len() + self.pending.len();

        let mut rows = column![].spacing(0).width(Fill);
        if self.pending.is_empty() && self.torrents.is_empty() {
            rows = rows.push(
                text("No downloads yet")
                    .size(16)
                    .style(|_| text::Style {
                        color: Some(Color::from_rgb8(0xc8, 0xc2, 0xb6)),
                    })
                    .width(Fill),
            );
        }
        let mut index = 0usize;
        for item in &self.pending {
            let selected = self.selected == Selected::Pending(item.source.clone());
            let status = if !item.error.is_empty() {
                "Error"
            } else if item.accepted {
                "Queued"
            } else {
                "Starting"
            };
            rows = rows.push(
                mouse_area(transfer_button(
                    TransferCells {
                        name: &item.name,
                        size: "—",
                        progress: None,
                        status,
                        down: "—",
                        up: "—",
                        peers: "—",
                        eta: "—",
                    },
                    selected,
                    index % 2 == 1,
                    Message::SelectPending(item.source.clone()),
                ))
                .on_right_press(Message::ContextPending(item.source.clone())),
            );
            index += 1;
        }
        for torrent in &self.torrents {
            let selected = self.selected == Selected::Torrent(torrent.id);
            let (size, progress, peers, eta) = if torrent.size == 0 {
                ("—".into(), None, "—".into(), "—".into())
            } else {
                (
                    wire::format_bytes(torrent.size),
                    Some(torrent.progress),
                    torrent.peers.to_string(),
                    format_eta(torrent),
                )
            };
            let status = status_label(torrent);
            rows = rows.push(
                mouse_area(transfer_button(
                    TransferCells {
                        name: &torrent.name,
                        size: &size,
                        progress,
                        status: &status,
                        down: &torrent.down,
                        up: &torrent.up,
                        peers: &peers,
                        eta: &eta,
                    },
                    selected,
                    index % 2 == 1,
                    Message::SelectTorrent(torrent.id),
                ))
                .on_right_press(Message::ContextTorrent(torrent.id)),
            );
            index += 1;
        }

        let selection = match &self.selected {
            Selected::Torrent(id) => self.torrents.iter().find(|row| row.id == *id).map(|row| {
                if row.error.is_empty() {
                    format!("{} · saved in {}", status_label(row), row.folder)
                } else {
                    format!(
                        "{} · saved in {}",
                        access::explain(Path::new(&row.folder), &row.error),
                        row.folder
                    )
                }
            }),
            Selected::Pending(source) => self
                .pending
                .iter()
                .find(|item| &item.source == source)
                .map(|item| {
                    if item.error.is_empty() {
                        format!("Saving to {}", self.download_folder.trim())
                    } else {
                        access::explain(Path::new(self.download_folder.trim()), &item.error)
                    }
                }),
            Selected::None => None,
        };

        let mut page = column![
            row![
                text("Keel").font(semibold()).size(22).style(copper),
                Space::new().width(12),
                text_input("Paste a magnet link or torrent path", &self.add_box)
                    .on_input(Message::AddBox)
                    .on_submit(Message::AddTyped)
                    .padding(10)
                    .size(15)
                    .width(Fill),
                primary("Add", Message::AddOpen),
                pause,
                remove,
                command("Share", Some(Message::ShareOpen)),
                Space::new().width(8),
                quiet("Uninstall", Message::AskUninstall),
            ]
            .spacing(8)
            .align_y(iced::Center),
            row![
                text("Save to").size(14),
                text_input("Save folder", &self.download_folder)
                    .on_input(Message::SaveFolder)
                    .padding(8)
                    .size(14)
                    .width(Fill),
                command("Browse", Some(Message::Browse(BrowseFor::Save))),
            ]
            .spacing(8)
            .align_y(iced::Center),
        ]
        .spacing(10)
        .width(Fill)
        .height(Fill);
        if let Some(warning) = self.folder_warning {
            page = page.push(text(warning).size(14).style(danger_text).width(Fill));
        }
        if !note.is_empty() {
            page = page.push(text(note).size(14).style(danger_text).width(Fill));
        }
        page = page.push(hairline());
        page = page.push(table_header());
        page = page.push(scrollable(rows).height(Fill).width(Fill));
        page = page.push(hairline());
        if let Some(line) = selection {
            page = page.push(muted(line));
        }
        if let Selected::Torrent(id) = self.selected {
            page = page.push(self.detail_panel(id));
        }
        page = page.push(muted(format!(
            "{}  ·  {} down  ·  {} up  ·  Remove keeps the files",
            if count == 1 {
                "1 torrent".to_owned()
            } else {
                format!("{count} torrents")
            },
            wire::format_rate(down as f64),
            wire::format_rate(up as f64),
        )));
        page.into()
    }

    fn detail_panel(&self, id: u64) -> Element<'_, Message> {
        let ready = self.details_for == Some(id);
        let trackers_tab = if self.detail_tab == DetailTab::Trackers {
            command("Trackers", None)
        } else {
            command("Trackers", Some(Message::ShowDetail(DetailTab::Trackers)))
        };
        let peers_tab = if self.detail_tab == DetailTab::Peers {
            command("Peers", None)
        } else {
            command("Peers", Some(Message::ShowDetail(DetailTab::Peers)))
        };
        let mut body = column![].spacing(4).width(Fill);
        if self.detail_tab == DetailTab::Trackers {
            if !ready {
                body = body.push(muted("Trackers appear in a moment."));
            } else if self.trackers.is_empty() {
                body = body.push(muted("No trackers on this torrent."));
            }
            for tracker in &self.trackers {
                body = body.push(flag_line(
                    &tracker.country,
                    format!("{}    {}", tracker.host, tracker.url),
                ));
            }
            if ready && !self.trackers.is_empty() {
                body = body.push(muted("Each address above is a tracker on this torrent."));
            }
        } else {
            if !ready {
                body = body.push(muted("Peers appear in a moment."));
            } else if self.peers.is_empty() {
                body = body.push(muted("No peers yet."));
            }
            for peer in &self.peers {
                let down = if peer.speeds_known {
                    wire::format_rate(peer.down_bps as f64)
                } else {
                    "—".into()
                };
                let up = if peer.speeds_known {
                    wire::format_rate(peer.up_bps as f64)
                } else {
                    "—".into()
                };
                body = body.push(flag_line(
                    &peer.country,
                    format!(
                        "{}    {}    {} down    {} up",
                        peer.addr, peer.client, down, up
                    ),
                ));
            }
            body = body.push(muted("Country data © DB-IP"));
        }
        column![
            row![
                text("Details").font(semibold()).size(14),
                trackers_tab,
                peers_tab
            ]
            .spacing(8)
            .align_y(iced::Center),
            scrollable(body).height(150).width(Fill),
        ]
        .spacing(6)
        .width(Fill)
        .into()
    }

    fn share(&self) -> Element<'_, Message> {
        let mut page = column![
            row![
                text("Share a file").font(semibold()).size(22).style(copper),
                Space::new().width(Fill),
                quiet("Back", Message::ShareBack),
            ]
            .align_y(iced::Center),
            muted("Create a torrent other people can open. Your files stay where they are."),
            row![
                text("File or folder").size(14).width(120),
                text_input("Full path", &self.share_source)
                    .on_input(Message::ShareSource)
                    .padding(8)
                    .size(14)
                    .width(Fill),
                command("Browse", Some(Message::Browse(BrowseFor::ShareSource))),
            ]
            .spacing(8)
            .align_y(iced::Center),
            row![
                text("Name").size(14).width(120),
                text_input("Shown in the magnet link", &self.share_name)
                    .on_input(Message::ShareName)
                    .padding(8)
                    .size(14)
                    .width(Fill),
            ]
            .spacing(8)
            .align_y(iced::Center),
            text("Trackers, one address on each line. Leave this empty to use DHT only.")
                .size(14),
            text_editor(&self.share_trackers)
                .on_action(Message::ShareTrackers)
                .height(90)
                .padding(8)
                .size(14),
            row![
                text("Write .torrent").size(14).width(120),
                text_input("Folder for the torrent file", &self.share_output)
                    .on_input(Message::ShareOutput)
                    .padding(8)
                    .size(14)
                    .width(Fill),
                command("Browse", Some(Message::Browse(BrowseFor::ShareOutput))),
            ]
            .spacing(8)
            .align_y(iced::Center),
            muted(
                "Pieces are 2 MiB. A file keeps its current name so seeding can find it. A folder is shared under the name above.",
            ),
            checkbox(self.share_seed)
                .label("Also seed")
                .on_toggle(Message::ShareSeed)
                .size(20),
            muted("Also seed shares the files from the folder they are already in."),
            row![
                primary("Create torrent", Message::ShareCreate),
                Space::new().width(Fill),
            ]
            .align_y(iced::Center),
        ]
        .spacing(10)
        .width(Fill);
        if self.share_busy {
            page = page.push(muted("Hashing the files."));
        }
        if !self.share_error.is_empty() {
            let hint = if self.share_output.trim().is_empty() {
                self.share_source.as_str()
            } else {
                self.share_output.trim()
            };
            page = page.push(
                text(access::explain(Path::new(hint), &self.share_error))
                    .size(14)
                    .style(danger_text),
            );
        }
        if let Some(created) = &self.share_result {
            let pieces = created.piece_length / (1024 * 1024);
            page = page.push(text(format!("Info hash  {}", created.infohash)).size(14));
            page = page.push(text(format!("Wrote  {}", created.torrent_path)).size(14));
            page = page.push(muted(format!("Piece size  {pieces} MiB")));
            page = page.push(command("Copy magnet", Some(Message::CopyCreated)));
        }
        scrollable(page).width(Fill).height(Fill).into()
    }

    fn card_overlay(&self) -> Option<Element<'_, Message>> {
        if self.delete_id.is_some() {
            return Some(self.delete_overlay());
        }
        if self.browser.is_some() {
            return Some(self.browser_overlay());
        }
        if self.add_open {
            return Some(self.add_overlay());
        }
        if self.menu.is_some() {
            return Some(self.menu_overlay());
        }
        None
    }

    fn delete_overlay(&self) -> Element<'_, Message> {
        let folder = self
            .delete_id
            .and_then(|id| self.torrents.iter().find(|row| row.id == id))
            .map(|row| row.folder.clone())
            .unwrap_or_else(|| "this torrent".to_owned());
        let card = column![
            text("Delete these files?").font(semibold()).size(20),
            body_text(folder),
            muted("Only this torrent's own files are removed. Other files in the folder stay."),
            row![
                quiet("Cancel", Message::CancelDelete),
                Space::new().width(Fill),
                danger("Delete files", Message::ConfirmDelete),
            ]
            .align_y(iced::Center),
        ]
        .spacing(12)
        .width(480);
        modal(Message::CancelDelete, card.into())
    }

    fn access_overlay(&self) -> Element<'_, Message> {
        let Some(problem) = &self.access else {
            return Space::new().into();
        };
        let (title, body, confirm, confirm_message): (String, String, &'static str, Message) =
            match problem {
                access::Problem::Mount { label, .. } => (
                    format!("Mount {label}"),
                    format!("{label} is unplugged or not mounted. Mount it to save files there."),
                    "Mount drive",
                    Message::AccessMount,
                ),
                access::Problem::Write { path } => (
                    "Allow saving".into(),
                    format!(
                        "Keel does not have permission to save files in {}. Allow writing for your account?",
                        path.display()
                    ),
                    "Allow writing",
                    Message::AccessAllow,
                ),
            };
        let mut card = column![text(title).font(semibold()).size(20), body_text(body),]
            .spacing(12)
            .width(520);
        if !self.access_note.is_empty() {
            card = card.push(text(self.access_note.clone()).size(14).style(danger_text));
        }
        card = card.push(
            row![
                quiet("Not now", Message::AccessDismiss),
                Space::new().width(Fill),
                primary(confirm, confirm_message),
            ]
            .align_y(iced::Center),
        );
        modal(Message::AccessDismiss, card.into())
    }

    fn add_overlay(&self) -> Element<'_, Message> {
        let mut card = column![
            text("Add a torrent").font(semibold()).size(20),
            text_input("Magnet link or torrent path", &self.add_draft)
                .on_input(Message::AddDraft)
                .on_submit(Message::AddStart)
                .padding(12)
                .size(15)
                .width(Fill),
            row![
                text("Save to").size(14),
                text_input("Save folder", &self.download_folder)
                    .on_input(Message::SaveFolder)
                    .padding(8)
                    .size(14)
                    .width(Fill),
                command("Browse", Some(Message::Browse(BrowseFor::Save))),
            ]
            .spacing(8)
            .align_y(iced::Center),
        ]
        .spacing(12)
        .width(560);
        if let Some(warning) = self.folder_warning {
            card = card.push(text(warning).size(14).style(danger_text).width(Fill));
        }
        card = card.push(
            row![
                quiet("Cancel", Message::AddClose),
                Space::new().width(Fill),
                primary("Start", Message::AddStart),
            ]
            .align_y(iced::Center),
        );
        modal(Message::AddClose, card.into())
    }

    fn browser_overlay(&self) -> Element<'_, Message> {
        let Some(browser) = &self.browser else {
            return Space::new().into();
        };
        let title = match browser.purpose {
            BrowseFor::Save => "Choose a save folder",
            BrowseFor::ShareSource => "Choose a file or folder to share",
            BrowseFor::ShareOutput => "Choose where to write the torrent file",
        };
        let mut places_row = row![].spacing(6);
        for place in &browser.places {
            places_row = places_row.push(place_button(place));
        }
        let mut listing = column![].spacing(0).width(Fill);
        if browser.entries.is_empty() {
            listing = listing.push(muted("This folder is empty."));
        }
        for (index, entry) in browser.entries.iter().enumerate() {
            let label = if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };
            let picked = browser.picked == Some(index);
            listing = listing.push(browser_row(label, index, picked));
        }
        let mut card = column![
            text(title).font(semibold()).size(20),
            places_row,
            text(browser.path.display().to_string()).size(13),
            scrollable(listing).height(280).width(Fill),
        ]
        .spacing(10)
        .width(640);
        if !browser.error.is_empty() {
            card = card.push(
                text(access::explain(&browser.path, &browser.error))
                    .size(14)
                    .style(danger_text),
            );
        }
        let mut actions = row![
            quiet("Up", Message::BrowseUp),
            quiet("Cancel", Message::BrowseClose),
            Space::new().width(Fill),
            primary("Use this folder", Message::BrowseUse),
        ]
        .spacing(8)
        .align_y(iced::Center);
        if browser.purpose == BrowseFor::ShareSource {
            actions = actions.push(primary("Use this file", Message::BrowseFile));
        }
        card = card.push(actions);
        modal(Message::BrowseClose, card.into())
    }

    fn menu_overlay(&self) -> Element<'_, Message> {
        let (x, y) = menu_origin(self.cursor);
        let mut items = column![].spacing(2).width(220);
        items = items.push(menu_item("Open destination", Message::OpenDestination));
        items = items.push(menu_item("Start", Message::ResumeSelected));
        if matches!(self.menu, Some(MenuTarget::Torrent(_))) {
            items = items.push(menu_item("Pause", Message::PauseSelected));
        }
        items = items.push(menu_item("Copy magnet", Message::CopyMagnet));
        items = items.push(menu_item("Remove", Message::RemoveSelected));
        if matches!(self.menu, Some(MenuTarget::Torrent(_))) {
            items = items.push(menu_item("Delete files", Message::AskDelete));
        }
        let card = container(items).padding(8).style(menu_card);
        stack([
            mouse_area(Space::new().width(Fill).height(Fill))
                .on_press(Message::MenuClose)
                .into(),
            column![
                Space::new().height(y),
                row![Space::new().width(x), opaque(card)].spacing(0),
            ]
            .into(),
        ])
        .width(Fill)
        .height(Fill)
        .into()
    }
}

fn same_transfer(row: &TorrentRow, name: &str, infohash: &str) -> bool {
    row.name == name
        || (!infohash.is_empty()
            && !row.infohash.is_empty()
            && row.infohash.eq_ignore_ascii_case(infohash))
}

fn table_header() -> Element<'static, Message> {
    row![
        head("Name", Fill),
        head("Size", 72),
        head("Progress", 128),
        head("Status", 108),
        head("Down", 84),
        head("Up", 84),
        head("Peers", 52),
        head("ETA", 64),
    ]
    .spacing(8)
    .padding([4, 8])
    .width(Fill)
    .into()
}

struct TransferCells<'a> {
    name: &'a str,
    size: &'a str,
    progress: Option<f32>,
    status: &'a str,
    down: &'a str,
    up: &'a str,
    peers: &'a str,
    eta: &'a str,
}

fn transfer_button(
    cells: TransferCells<'_>,
    selected: bool,
    stripe: bool,
    message: Message,
) -> Element<'static, Message> {
    let progress: Element<'static, Message> = match cells.progress {
        Some(value) => {
            let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u16;
            row![
                progress_bar(0.0..=1.0, value).girth(6).length(70),
                text(format!("{percent}%")).size(13),
            ]
            .spacing(6)
            .align_y(iced::Center)
            .width(128)
            .into()
        }
        None => cell("—", 128),
    };
    button(
        row![
            text(cells.name.to_owned()).size(14).width(Fill),
            cell(cells.size, 72),
            progress,
            cell(cells.status, 108),
            cell(cells.down, 84),
            cell(cells.up, 84),
            cell(cells.peers, 52),
            cell(cells.eta, 64),
        ]
        .spacing(8)
        .align_y(iced::Center),
    )
    .width(Fill)
    .padding([7, 8])
    .on_press(message)
    .style(move |_theme, status| transfer_style(selected, stripe, status))
    .into()
}

fn transfer_style(selected: bool, stripe: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = if selected {
        Color::from_rgb8(0x3a, 0x2c, 0x24)
    } else if hovered {
        Color::from_rgb8(0x22, 0x25, 0x2d)
    } else if stripe {
        Color::from_rgb8(0x18, 0x1b, 0x22)
    } else {
        Color::from_rgb8(0x1b, 0x1e, 0x26)
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: Color::from_rgb8(0xf4, 0xf0, 0xe8),
        border: Border {
            color: Color::from_rgb8(0x32, 0x36, 0x40),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

fn hairline() -> Element<'static, Message> {
    container(Space::new().height(1).width(Fill))
        .width(Fill)
        .style(rule)
        .into()
}

fn head(label: &'static str, width: impl Into<iced::Length>) -> Element<'static, Message> {
    text(label)
        .size(12)
        .font(semibold())
        .width(width)
        .style(|_| text::Style {
            color: Some(Color::from_rgb8(0xc8, 0xc2, 0xb6)),
        })
        .into()
}

fn cell(label: impl Into<String>, width: impl Into<iced::Length>) -> Element<'static, Message> {
    text(label.into()).size(14).width(width).into()
}

fn command(label: &'static str, message: Option<Message>) -> Element<'static, Message> {
    let mut control = button(text(label)).padding([8, 12]).style(button::text);
    if let Some(message) = message {
        control = control.on_press(message);
    }
    control.into()
}

fn status_label(row: &TorrentRow) -> String {
    if !row.error.is_empty() || row.state == "error" {
        "Error".into()
    } else if row.size == 0 && row.state != "paused" {
        "Metadata".into()
    } else if row.finished && row.state == "paused" {
        "Finished".into()
    } else if row.finished {
        "Seeding".into()
    } else {
        match row.state.as_str() {
            "paused" => "Paused".into(),
            "live" => "Downloading".into(),
            "initializing" => "Starting".into(),
            _ => "Working".into(),
        }
    }
}

fn format_eta(row: &TorrentRow) -> String {
    if row.finished || row.down_bps == 0 || row.size <= row.done {
        return "—".into();
    }
    let seconds = (row.size - row.done) / row.down_bps.max(1);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn wizard<'a>(body: Element<'a, Message>, actions: Element<'a, Message>) -> Element<'a, Message> {
    column![body, Space::new().height(Fill), actions]
        .spacing(24)
        .width(Fill)
        .height(Fill)
        .into()
}

fn kicker(label: &'static str) -> Element<'static, Message> {
    text(label).size(13).style(copper).into()
}

fn heading(label: impl Into<String>) -> Element<'static, Message> {
    text(label.into()).font(semibold()).size(32).into()
}

fn body_text(label: impl Into<String>) -> Element<'static, Message> {
    text(label.into()).size(17).width(Fill).into()
}

fn muted(label: impl Into<String>) -> Element<'static, Message> {
    text(label.into())
        .size(15)
        .style(|_| text::Style {
            color: Some(Color::from_rgb8(0xc8, 0xc2, 0xb6)),
        })
        .into()
}

fn copper(theme: &Theme) -> text::Style {
    let _ = theme;
    text::Style {
        color: Some(Color::from_rgb8(0xe0, 0x8a, 0x4f)),
    }
}

fn danger_text(_theme: &Theme) -> text::Style {
    text::Style {
        color: Some(Color::from_rgb8(0xff, 0xb1, 0xa6)),
    }
}

fn semibold() -> Font {
    Font {
        weight: iced::font::Weight::Semibold,
        ..Font::with_name("Inter")
    }
}

fn primary(label: &'static str, message: Message) -> Element<'static, Message> {
    button(text(label))
        .on_press(message)
        .padding([12, 22])
        .style(button::primary)
        .into()
}

fn quiet(label: &'static str, message: Message) -> Element<'static, Message> {
    button(text(label))
        .on_press(message)
        .padding([12, 18])
        .style(button::text)
        .into()
}

fn danger(label: &'static str, message: Message) -> Element<'static, Message> {
    button(text(label))
        .on_press(message)
        .padding([12, 22])
        .style(button::danger)
        .into()
}

fn check_row(
    label: &'static str,
    help: impl Into<String>,
    checked: bool,
    toggle: fn(bool) -> Message,
) -> Element<'static, Message> {
    column![
        checkbox(checked).label(label).on_toggle(toggle).size(20),
        text(help.into()).size(15).style(|_| text::Style {
            color: Some(Color::from_rgb8(0xc8, 0xc2, 0xb6)),
        }),
    ]
    .spacing(4)
    .into()
}

fn rule(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(0x32, 0x36, 0x40))),
        ..Default::default()
    }
}

fn startup_folder(layout: &Layout) -> (String, Option<access::Problem>) {
    if let Some(path) = settings::remembered_download_dir(layout) {
        if let Some(problem) = access::inspect_live(&path) {
            let keep = matches!(problem, access::Problem::Mount { .. }) || path.is_dir();
            if keep {
                return (path.display().to_string(), Some(problem));
            }
        } else if path.is_dir() {
            return (path.display().to_string(), None);
        }
    }
    (
        settings::initial_download_dir(layout).display().to_string(),
        None,
    )
}

fn folder_warning_for(folder: &str) -> Option<&'static str> {
    let kind = places::filesystem_kind(Path::new(folder.trim()));
    places::fat32_warning(kind.as_deref())
}

fn tracker_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn open_folder(path: &str) -> Result<(), String> {
    let folder = Path::new(path);
    if let Some(problem) = access::inspect_live(folder) {
        return Err(problem.sentence());
    }
    if !folder.is_dir() {
        return Err(format!("{path} is not there."));
    }
    let mut child = std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| access::explain(folder, &format!("Could not open {path}: {err}")))?;
    let status = child
        .wait()
        .map_err(|err| access::explain(folder, &format!("Could not open {path}: {err}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Could not open {path}."))
    }
}

fn menu_origin(cursor: iced::Point) -> (f32, f32) {
    let x = cursor.x.clamp(8.0, 720.0);
    let y = if cursor.y > 460.0 {
        (cursor.y - 240.0).max(8.0)
    } else {
        cursor.y
    };
    (x, y)
}

fn menu_item(label: &'static str, message: Message) -> Element<'static, Message> {
    button(text(label).size(15))
        .width(Fill)
        .padding([8, 12])
        .on_press(message)
        .style(button::text)
        .into()
}

fn menu_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(0x1b, 0x1e, 0x26))),
        border: Border {
            color: Color::from_rgb8(0x32, 0x36, 0x40),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

fn modal(close: Message, card: Element<'_, Message>) -> Element<'_, Message> {
    stack([
        mouse_area(
            container(Space::new().width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.45))),
                    ..Default::default()
                }),
        )
        .on_press(close)
        .into(),
        container(opaque(card))
            .width(Fill)
            .height(Fill)
            .align_x(iced::Center)
            .align_y(iced::Center)
            .into(),
    ])
    .width(Fill)
    .height(Fill)
    .into()
}

fn place_button(place: &Place) -> Element<'static, Message> {
    let path = place.path.display().to_string();
    button(text(place.label.clone()).size(14))
        .padding([6, 10])
        .on_press(Message::BrowsePlace(path))
        .style(button::text)
        .into()
}

fn browser_row(label: String, index: usize, picked: bool) -> Element<'static, Message> {
    button(text(label).size(14))
        .width(Fill)
        .padding([6, 8])
        .on_press(Message::BrowseActivate(index))
        .style(move |_theme, status| transfer_style(picked, false, status))
        .into()
}

fn flag_line(code: &str, label: String) -> Element<'_, Message> {
    let mark: Element<'_, Message> = if code.is_empty() {
        Space::new().width(22).height(14).into()
    } else {
        row![flags::icon(code), text(code.to_owned()).size(12)]
            .spacing(4)
            .align_y(iced::Center)
            .into()
    };
    row![mark, text(label).size(14).width(Fill)]
        .spacing(8)
        .align_y(iced::Center)
        .width(Fill)
        .into()
}

fn keel_theme() -> Theme {
    static THEME: LazyLock<Theme> = LazyLock::new(|| {
        Theme::custom(
            "Keel",
            iced::theme::Palette {
                background: Color::from_rgb8(0x14, 0x16, 0x1c),
                text: Color::from_rgb8(0xf4, 0xf0, 0xe8),
                primary: Color::from_rgb8(0xe0, 0x8a, 0x4f),
                success: Color::from_rgb8(0x8f, 0xaf, 0x72),
                warning: Color::from_rgb8(0xe0, 0xb1, 0x5c),
                danger: Color::from_rgb8(0xd4, 0x54, 0x4a),
            },
        )
    });
    THEME.clone()
}

#[cfg(test)]
mod wizard_flow {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        Back, BrowseFor, Browser, Keel, Launch, Message, Screen, Selected, REGULAR, SEMIBOLD,
    };
    use crate::cli::Mode;
    use crate::install;
    use crate::layout::Layout;
    use crate::settings;
    use crate::wire::TorrentRow;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        home: PathBuf,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.home);
        }
    }

    fn scratch() -> Scratch {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("keel-ui-{nanos}-{n}"));
        fs::create_dir_all(&home).unwrap();
        Scratch { home }
    }

    fn wizard_size() -> iced::Size {
        iced::Size::new(760.0, 640.0)
    }

    fn library_size() -> iced::Size {
        iced::Size::new(980.0, 680.0)
    }

    fn settings() -> iced::Settings {
        iced::Settings {
            fonts: vec![REGULAR.into(), SEMIBOLD.into()],
            default_font: iced::Font::with_name("Inter"),
            default_text_size: iced::Pixels(17.0),
            ..Default::default()
        }
    }

    fn launch(home: &Path, mode: Mode, open: Vec<String>) -> Keel {
        Keel::new(Launch {
            mode,
            layout: Layout::new(home),
            open,
            source_binary: std::env::current_exe().unwrap(),
            hold: None,
        })
    }

    fn click(keel: &mut Keel, label: &str, size: iced::Size) {
        let messages = {
            let mut ui = iced_test::Simulator::with_size(settings(), size, keel.view());
            ui.click(label)
                .unwrap_or_else(|err| panic!("click {label}: {err}"));
            ui.into_messages().collect::<Vec<_>>()
        };
        assert!(!messages.is_empty(), "click {label} produced no message");
        for message in messages {
            let _task = keel.update(message);
        }
    }

    fn must_show(keel: &Keel, size: iced::Size, label: &str) {
        let mut ui = iced_test::Simulator::with_size(settings(), size, keel.view());
        ui.find(label)
            .unwrap_or_else(|err| panic!("missing {label}: {err}"));
    }

    fn must_hide(keel: &Keel, size: iced::Size, label: &str) {
        let mut ui = iced_test::Simulator::with_size(settings(), size, keel.view());
        if ui.find(label).is_ok() {
            panic!("should not show {label}");
        }
    }

    fn stamp(path: &Path) -> Option<SystemTime> {
        fs::metadata(path).and_then(|meta| meta.modified()).ok()
    }

    #[test]
    fn clicks_through_install_and_uninstall_without_touching_the_real_home() {
        let real_home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let watched = [
            real_home.join(".local/share/keel"),
            real_home.join(".local/share/applications/keel.desktop"),
            real_home.join(".config/mimeapps.list"),
            real_home.join(".local/bin/keel"),
        ];
        let before: Vec<_> = watched.iter().map(|path| stamp(path)).collect();

        let scratch = scratch();
        fs::create_dir_all(scratch.home.join("Downloads")).unwrap();
        fs::write(scratch.home.join("Downloads/keep.iso"), b"not a keel file").unwrap();

        let mut keel = launch(&scratch.home, Mode::Wizard, Vec::new());
        let size = wizard_size();
        must_show(&keel, size, "Install Keel");
        must_show(
            &keel,
            size,
            "The library downloads a torrent into a folder you choose.",
        );

        click(&mut keel, "Continue", size);
        assert!(matches!(keel.screen, Screen::Setup));
        must_show(&keel, size, "Where should Keel live?");

        click(
            &mut keel,
            "Open .torrent files and magnet links with Keel",
            size,
        );
        assert!(!keel.register_handlers);
        click(
            &mut keel,
            "Open .torrent files and magnet links with Keel",
            size,
        );
        assert!(keel.register_handlers);

        let _ = keel.update(Message::FolderChanged("/tmp".into()));
        click(&mut keel, "Install", size);
        assert!(matches!(
            keel.screen,
            Screen::Failed {
                back: Back::Setup,
                ..
            }
        ));
        must_show(&keel, size, "Choose a different folder");

        click(&mut keel, "Back", size);
        assert!(matches!(keel.screen, Screen::Setup));

        let install_dir = keel.layout.default_install_dir();
        let _ = keel.update(Message::FolderChanged(install_dir.display().to_string()));
        click(&mut keel, "Install", size);
        assert!(matches!(
            keel.screen,
            Screen::Working {
                title: "Installing Keel",
                ..
            }
        ));

        let report = install::install(&install::Request {
            layout: keel.layout.clone(),
            source_binary: keel.source_binary.clone(),
            install_dir: install_dir.clone(),
            register_handlers: keel.register_handlers,
            add_to_path: keel.add_to_path,
        })
        .unwrap();
        let _ = keel.update(Message::Installed(Ok(report.clone())));
        assert!(matches!(keel.screen, Screen::Installed(_)));
        must_show(&keel, size, "Keel is installed");

        assert!(report.binary.is_file());
        assert!(install_dir.join("keel.png").is_file());
        assert!(install_dir.join("install.txt").is_file());
        assert!(keel.layout.desktop_path().is_file());
        assert!(keel.layout.bin_link().is_symlink());
        let mime = fs::read_to_string(keel.layout.mimeapps_path()).unwrap();
        assert!(mime.contains("application/x-bittorrent=keel.desktop;"));
        assert!(mime.contains("x-scheme-handler/magnet=keel.desktop;"));

        fs::write(install_dir.join("notes.txt"), b"keep me").unwrap();

        let wide = library_size();
        let empty = launch(&scratch.home, Mode::Library, Vec::new());
        must_show(&empty, wide, "No downloads yet");

        let opened = "magnet:?xt=urn:btih:4E101BF616AD6C7B0F9D5F86B92457A2CC0A9C4C&dn=I+Want+Your+Sex+%282026%29+%281080p+BluRay+x265+10bit+EAC3+Atmos+7.1+Celdra%29+%5BQxR%5D";
        let mut library = launch(&scratch.home, Mode::Library, vec![opened.into()]);
        must_show(
            &library,
            wide,
            "I Want Your Sex (2026) (1080p BluRay x265 10bit EAC3 Atmos 7.1 Celdra) [QxR]",
        );
        must_show(&library, wide, "Starting");
        must_show(&library, wide, "Save to");
        must_show(&library, wide, "Progress");
        must_show(&library, wide, "Peers");
        must_show(&library, wide, "Pause");
        must_show(&library, wide, "Remove");
        must_show(
            &library,
            wide,
            "1 torrent  ·  0 B/s down  ·  0 B/s up  ·  Remove keeps the files",
        );
        must_hide(&library, wide, "magnet:?");
        must_hide(&library, wide, "%282026%29");
        must_hide(&library, wide, "No downloads yet");
        must_hide(&library, wide, "Download");
        must_hide(&library, wide, "Delete files");

        click(&mut library, "Uninstall", wide);
        assert!(matches!(library.screen, Screen::UninstallConfirm));
        must_show(&library, wide, "Downloaded files are left where they are.");

        click(&mut library, "Remove Keel", wide);
        assert!(matches!(
            library.screen,
            Screen::Working {
                title: "Removing Keel",
                ..
            }
        ));
        let removed = install::uninstall(&library.layout).unwrap();
        let _ = library.update(Message::Uninstalled(Ok(removed)));
        assert!(matches!(library.screen, Screen::Uninstalled(_)));
        must_show(&library, wide, "Downloaded files were not touched.");

        assert_eq!(fs::read(install_dir.join("notes.txt")).unwrap(), b"keep me");
        assert_eq!(
            fs::read(scratch.home.join("Downloads/keep.iso")).unwrap(),
            b"not a keel file"
        );
        assert!(!install_dir.join("keel").exists());
        assert!(!install_dir.join("keel.png").exists());
        assert!(!install_dir.join("install.txt").exists());
        assert!(!library.layout.desktop_path().exists());
        assert!(!library.layout.bin_link().exists());
        let mime = fs::read_to_string(library.layout.mimeapps_path()).unwrap_or_default();
        assert!(!mime.contains("keel.desktop"));
        assert!(matches!(
            install::uninstall(&library.layout),
            Err(install::UninstallError::NotInstalled)
        ));

        let after: Vec<_> = watched.iter().map(|path| stamp(path)).collect();
        assert_eq!(before, after, "the real home directory was modified");
    }

    #[test]
    fn choosing_a_folder_remembers_it_and_a_link_is_refused() {
        let scratch = scratch();
        let movies = scratch.home.join("movies");
        fs::create_dir_all(&movies).unwrap();
        let mut keel = launch(&scratch.home, Mode::Library, Vec::new());
        keel.browser = Some(Browser {
            purpose: BrowseFor::Save,
            path: movies.clone(),
            entries: Vec::new(),
            places: Vec::new(),
            picked: None,
            error: String::new(),
        });
        let _ = keel.update(Message::BrowseUse);
        let saved = movies.canonicalize().unwrap();
        assert_eq!(PathBuf::from(&keel.download_folder), saved);
        assert_eq!(settings::load_download_dir(&keel.layout).unwrap(), saved);
        assert!(keel.browser.is_none());

        let link = scratch.home.join("linked");
        std::os::unix::fs::symlink(&saved, &link).unwrap();
        keel.browser = Some(Browser {
            purpose: BrowseFor::Save,
            path: link,
            entries: Vec::new(),
            places: Vec::new(),
            picked: None,
            error: String::new(),
        });
        let _ = keel.update(Message::BrowseUse);
        let browser = keel.browser.as_ref().expect("the link stays open");
        assert!(!browser.error.is_empty());
        assert_eq!(PathBuf::from(&keel.download_folder), saved);
    }

    #[test]
    fn right_click_menu_offers_destination_pause_remove_and_delete() {
        let scratch = scratch();
        let mut keel = launch(&scratch.home, Mode::Library, Vec::new());
        keel.torrents.push(TorrentRow {
            id: 7,
            name: "Sample".into(),
            infohash: "ab".repeat(20),
            state: "live".into(),
            progress: 0.5,
            done: 10,
            size: 20,
            down: "0 B/s".into(),
            up: "0 B/s".into(),
            down_bps: 0,
            up_bps: 0,
            peers: 1,
            error: String::new(),
            folder: scratch.home.display().to_string(),
            finished: false,
        });
        keel.selected = Selected::Torrent(7);
        let wide = library_size();
        must_hide(&keel, wide, "Delete files");
        let _ = keel.update(Message::ContextTorrent(7));
        must_show(&keel, wide, "Open destination");
        must_show(&keel, wide, "Pause");
        must_show(&keel, wide, "Remove");
        must_show(&keel, wide, "Delete files");
        must_show(&keel, wide, "Country data © DB-IP");
    }

    #[test]
    fn a_missing_drive_asks_to_mount_and_a_locked_folder_asks_to_allow_writing() {
        let home = scratch();
        let wide = library_size();
        let missing = Path::new("/run/media/x1/KeelMissingDrive");
        settings::save_download_dir(&Layout::new(&home.home), missing).unwrap();
        let keel = launch(&home.home, Mode::Library, Vec::new());
        assert_eq!(keel.download_folder, missing.display().to_string());
        must_show(&keel, wide, "Mount drive");
        must_show(&keel, wide, "Not now");
        must_hide(&keel, wide, "os error");
        must_hide(&keel, wide, "Permission denied");
        assert!(keel.access.is_some());
        assert!(!missing.exists());

        let gone = home.home.join("no-such-folder");
        settings::save_download_dir(&Layout::new(&home.home), &gone).unwrap();
        let fallback = launch(&home.home, Mode::Library, Vec::new());
        assert!(fallback.download_folder.ends_with("Downloads"));
        assert!(fallback.access.is_none());

        let locked = home.home.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let mut keel = launch(&home.home, Mode::Library, Vec::new());
        keel.show_problem(&locked, "Permission denied (os error 13)");
        must_show(&keel, wide, "Allow writing");
        must_hide(&keel, wide, "Mount drive");
        must_hide(&keel, wide, "os error");
        must_hide(&keel, wide, "Permission denied");
        assert!(
            !keel.action_error.contains("os error"),
            "{}",
            keel.action_error
        );
        assert!(
            !keel.action_error.contains("Permission denied"),
            "{}",
            keel.action_error
        );

        let fresh = scratch();
        let mut plain = launch(&fresh.home, Mode::Library, Vec::new());
        plain.show_problem(
            Path::new("/run/media/x1/KeelMissingDrive/movies"),
            "Permission denied (os error 13)",
        );
        must_show(&plain, wide, "Mount drive");
        must_hide(&plain, wide, "os error");
        must_hide(&plain, wide, "Permission denied");
        assert!(
            !plain.action_error.contains("os error"),
            "{}",
            plain.action_error
        );
        assert!(
            !plain.action_error.contains("Permission denied"),
            "{}",
            plain.action_error
        );
        assert!(!Path::new("/run/media/x1/KeelMissingDrive").exists());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    }
}
