//! Command line for the installer and the library window.

pub const HELP: &str = "\
Keel 0.1.0

Usage:
  keel                 Install Keel, or open the library if this copy is installed
  keel --install       Open the installer
  keel --uninstall     Remove the user-account installation
  keel --app           Open the library
  keel <file-or-magnet>...
                       Open the library and download those torrents
  keel --engine        Run the download engine (the library starts this)

The installer copies Keel into your home folder, adds an application menu
entry, and can register .torrent files and magnet links. It does not need
an administrator password. Uninstall leaves downloaded files where they are
and stops the download engine.

KEEL_HOME, when set to a folder, is used as the home directory. This is for
trying the installer without writing into your real home folder.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Wizard,
    Library,
    Uninstall,
    Engine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub help: bool,
    pub mode: Option<Mode>,
    pub open: Vec<String>,
}

pub fn parse<I, S>(args: I) -> Result<Args, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut parsed = Args {
        help: false,
        mode: None,
        open: Vec::new(),
    };

    for arg in args {
        let arg = arg.as_ref();
        let mode = match arg {
            "--help" | "-h" => {
                parsed.help = true;
                continue;
            }
            "--install" | "--wizard" => Some(Mode::Wizard),
            "--uninstall" => Some(Mode::Uninstall),
            "--app" => Some(Mode::Library),
            "--engine" => Some(Mode::Engine),
            other if other.starts_with('-') => {
                return Err(format!("Unknown option {other}."));
            }
            other => {
                parsed.open.push(other.to_owned());
                continue;
            }
        };
        if parsed.mode.is_some() {
            return Err("Use only one of --install, --uninstall, --app, and --engine.".into());
        }
        parsed.mode = mode;
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_invocation_has_no_mode() {
        let args = parse(Vec::<String>::new()).unwrap();
        assert!(args.mode.is_none());
        assert!(args.open.is_empty());
    }

    #[test]
    fn files_and_magnets_are_kept() {
        let args = parse(["a.torrent", "magnet:?xt=urn:btih:abc"]).unwrap();
        assert_eq!(args.open, ["a.torrent", "magnet:?xt=urn:btih:abc"]);
        assert!(args.mode.is_none());
    }

    #[test]
    fn rejects_two_modes_and_unknown_flags() {
        assert!(parse(["--install", "--uninstall"]).is_err());
        assert!(parse(["--nope"]).is_err());
    }

    #[test]
    fn engine_is_a_mode() {
        let args = parse(["--engine"]).unwrap();
        assert_eq!(args.mode, Some(Mode::Engine));
        assert!(args.open.is_empty());
    }
}
