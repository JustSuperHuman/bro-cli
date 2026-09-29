//! bro — the agentic terminal workspace for Claude Code, Codex, Pi and omp.
//!
//! `main` parses the command line into a [`cli::Command`] and dispatches; the TUI lives in [`app`]. All calls
//! into bro-core / bro-proxy / bro-bridge go through [`services`] (background threads + panic guards).
//! Core derived from z4-oriel (MIT).

mod alerts;
mod archive;
mod app;
mod cli;
mod continue_picker;
mod clip;
mod folder;
mod fuzzy;
mod help;
mod instance;
mod icons;
mod keymap;
mod launcher;
mod layout;
mod palette;
mod pane;
mod projects;
mod proxy_cmd;
mod panes;
mod recents;
mod services;
mod sidebar;
mod testkit;
mod theme;
mod ui;
mod util;
mod views;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args) {
        Ok(cli::Command::Help) => {
            print!("{}", cli::HELP.replace("{version}", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Version) => {
            println!("bro {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Proxy(args)) => match proxy_cmd::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro proxy: {e:#}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Tui { demo, new, dir }) => match run_tui(demo, new, dir) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro: {e:#}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("bro: {e}\n\nrun `bro --help` for usage");
            ExitCode::from(2)
        }
    }
}

/// Set up the terminal, start services, run the app, restore the terminal.
fn run_tui(demo: bool, new: bool, dir: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    use crossterm::{event, execute, terminal};
    // the folder bro was opened in (or `bro <folder>`) becomes the current project
    let dir = match dir {
        Some(d) => util::strip_verbatim(std::fs::canonicalize(&d).map_err(|_| anyhow::anyhow!("{} isn't a folder", d.display()))?),
        None => std::env::current_dir()?,
    };
    // already running? hand the folder over and get out of the way
    if !demo && !new && instance::hand_off(&dir).unwrap_or(false) {
        println!("bro is already running — opened {} there (bro --new for a second one)", dir.display());
        return Ok(());
    }
    services::guard::install_panic_hook();
    util::probe_local_offset();
    // demo mode never reads or writes your real settings
    let (settings, problem) = if demo { (services::fallback_settings(), None) } else { services::load_settings() };
    let (tx, rx) = std::sync::mpsc::channel();
    // drop keystrokes typed before bro started (e.g. the Enter that launched it)
    while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
    let svc = if demo { services::Services::demo(settings, tx.clone()) } else { services::Services::start(settings, tx.clone()) };

    terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen, event::EnableMouseCapture, event::EnableBracketedPaste, event::EnableFocusChange)?;
    // kitty keyboard protocol where supported (Ghostty, kitty, WezTerm, foot…) so ctrl+tab and friends are
    // distinguishable; Windows reads console key events directly and doesn't need it
    #[cfg(not(windows))]
    let _ = execute!(stdout, event::PushKeyboardEnhancementFlags(event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    services::guard::TUI_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = match ratatui::Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            services::guard::restore_terminal();
            return Err(e.into());
        }
    };

    let input_tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(ev) = event::read() {
            if input_tx.send(pane::Event::Input(ev)).is_err() {
                break;
            }
        }
    });

    let _instance = if demo { None } else { instance::serve(tx.clone()) };
    let mut app = app::App::new(svc, tx, app::Opts { demo, fixed_demo: false, load_recents: !demo });
    if !demo {
        app.start_in(dir);
    }
    if let Some(p) = problem.filter(|p| !services::guard::is_unimplemented(p)) {
        app.toast(alerts::Kind::Error, format!("settings: {p} — using defaults"));
    }
    let res = app.run(&mut term, rx);
    services::guard::restore_terminal();
    res
}
