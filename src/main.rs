//! Keel. Linux installer, library window, and a separate download process.

mod access;
mod cli;
mod engine;
mod flags;
mod geo;
mod icon;
mod inbox;
mod install;
mod job;
mod layout;
mod mime;
mod places;
mod settings;
mod source;
mod speed;
mod ui;
mod wire;

fn main() -> iced::Result {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("keel: {err}");
            eprintln!("Run keel --help for usage.");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", cli::HELP);
        return Ok(());
    }

    let layout = match layout::Layout::from_env() {
        Ok(layout) => layout,
        Err(err) => {
            eprintln!("keel: {err}");
            std::process::exit(1);
        }
    };
    let source_binary = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("keel: could not find the running program: {err}");
            std::process::exit(1);
        }
    };

    let open = source::normalize_args(args.open);
    let mode = args
        .mode
        .unwrap_or(if open.is_empty() && !install::running_from_install() {
            cli::Mode::Wizard
        } else {
            cli::Mode::Library
        });

    if mode == cli::Mode::Engine {
        if let Err(err) = engine::serve(&layout, true) {
            eprintln!("keel: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let mut hold = None;
    if mode == cli::Mode::Library {
        match inbox::try_claim(&layout) {
            Ok(inbox::Claim::Owner(lock)) => hold = Some(std::sync::Arc::new(lock)),
            Ok(inbox::Claim::Busy) if !open.is_empty() => {
                if let Err(err) = inbox::append(&layout, &open) {
                    eprintln!("keel: {err}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            Ok(inbox::Claim::Busy) => {}
            Err(err) => {
                eprintln!("keel: {err}");
                std::process::exit(1);
            }
        }
    }

    ui::run(ui::Launch {
        mode,
        layout,
        open,
        source_binary,
        hold,
    })
}
