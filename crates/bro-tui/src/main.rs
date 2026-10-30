//! bro — the agentic terminal workspace for Claude Code, Codex, Pi and omp.
//!
//! `main` parses the command line into a [`cli::Command`] and dispatches; the TUI lives in [`app`]. All calls
//! into bro-core / bro-proxy / bro-bridge go through [`services`] (background threads + panic guards).
//! Core derived from z4-oriel (MIT).

mod agent_files;
mod alerts;
mod archive;
mod app;
mod card;
mod devprobe;
mod cli;
mod continue_picker;
mod clip;
#[cfg(windows)]
mod console_nav;
mod editor;
mod env_cmd;
mod gemini_cmd;
mod file_view;
mod focus;
mod folder;
mod fonts;
mod fuzzy;
mod help;
mod instance;
mod icons;
mod keymap;
mod maker;
mod launcher;
mod layout;
mod palette;
mod pane;
mod paste_burst;
mod perf;
mod priority;
mod quips;
mod projects;
mod install_cmd;
mod open_cmd;
mod ctl_cmd;
mod proxy_cmd;
mod panes;
mod orchestrator_state;
mod recents;
mod services;
mod sidebar;
mod team_board;
mod skin;
mod skills_cmd;
mod host_cmd;
mod testkit;
mod textarea;
mod theme;
mod ui;
mod util;
mod views;
mod watchdog;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // bro-app opened us in the Just Terminal fork: its font switching (OSC 50) is available
    if let Some(i) = args.iter().position(|a| a == "--app-window") {
        args.remove(i);
        fonts::APP_WINDOW.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // `bro-cli` (`--tui`, or BRO_UI=tui): the terminal UI; so does the terminal fork hosting bro (`--app-window`)
    let tui = take_tui_flag(&mut args) || fonts::APP_WINDOW.load(std::sync::atomic::Ordering::Relaxed);
    // `--view`: another window on the bro that's running (its sessions, to watch and type into)
    if let Some(i) = args.iter().position(|a| a == "--view") {
        args.remove(i);
        services::host::set_role(bro_host::Role::Viewer);
    }
    // `bro gui [folder] [--new] [--demo]`: the desktop app even where bro would pick the terminal UI
    #[cfg(feature = "gui")]
    let gui = app::gui::take_flag(&mut args);
    // `--env <file|url> [--save]`: into bro's environment before any thread starts, so everything inherits it
    let env = match env_cmd::take_flags(&mut args) {
        Ok(env) => env,
        Err(e) => {
            eprintln!("bro: {e}\n\nrun `bro env --help` for usage");
            return ExitCode::from(2);
        }
    };
    if !env.sources.is_empty() {
        if let Err(e) = env_cmd::run_flags(&env) {
            eprintln!("bro: {e}");
            return ExitCode::FAILURE;
        }
        // `bro --env x --save` on its own only saves
        if env.save && args.is_empty() {
            return ExitCode::SUCCESS;
        }
    }
    match cli::parse(&args) {
        Ok(cli::Command::Help) => {
            print!("{}", cli::HELP.replace("{version}", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Version) => {
            println!("bro {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Claude Code runs this as one of bro's hooks: the event on stdin, bro's answer (if any) on stdout. Whatever
        // goes wrong, the call goes ahead as it is: nothing printed, exit 0.
        Ok(cli::Command::Hook(args)) => {
            // `bro hook peon play <file> <volume>`: the player a sound hook started (no stdin)
            if args.get(1).map(String::as_str) == Some("play") && args.first().map(String::as_str) == Some("peon") {
                if let Some((file, volume)) = bro_core::peon::parse_play(&args[2..]) {
                    let _ = bro_voice::sound::play_file(&file, volume);
                }
                return ExitCode::SUCCESS;
            }
            // `bro hook sound question|ticks …`: the Claude mod's question chime and countdown (no stdin)
            if args.first().map(String::as_str) == Some("sound") {
                let _ = bro_voice::cue::play(&args[1..]);
                return ExitCode::SUCCESS;
            }
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
            let answer = match args.first().map(String::as_str) {
                Some("peers") => bro_core::peers::run_hook(&args[1..], &input),
                Some("peon") => {
                    // decide here, play from a process of its own: the agent doesn't wait on the sound
                    if let (Some(play), Ok(exe)) = (bro_core::peon::run_hook(&args[1..], &input), std::env::current_exe()) {
                        let _ = bro_core::peon::spawn_player(&exe, &play);
                    }
                    None
                }
                _ => None,
            };
            if let Some(answer) = answer {
                println!("{answer}");
            }
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Proxy(args)) => match proxy_cmd::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro proxy: {e:#}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::CtlHelp) => {
            print!("{}", ctl_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Ctl(args)) => {
            let ctx = open_cmd::Context::from_env();
            match ctl_cmd::run(&args, ctx.bridge.as_deref()) {
                Ok(out) => {
                    println!("{}", out.trim_end());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("bro ctl: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(cli::Command::OpenHelp) => {
            print!("{}", open_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Open(args)) => match open_cmd::run(&args, &open_cmd::Context::from_env()) {
            Ok(done) => {
                println!("{done}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bro open: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::EnvHelp) => {
            print!("{}", env_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Env(cmd)) => match env_cmd::run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro env: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Imagine) => match install_cmd::open_imagine() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro imagine: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::SkillsHelp) => {
            print!("{}", skills_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Background) => match run_background() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro background: {e:#}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Host(cmd)) => match host_cmd::run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro host: {e:#}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Skills(cmd)) => match skills_cmd::run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro skills: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::GeminiHelp) => {
            print!("{}", gemini_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Gemini(cmd)) => match gemini_cmd::run(cmd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro gemini: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::InstallHelp) => {
            print!("{}", install_cmd::HELP);
            ExitCode::SUCCESS
        }
        Ok(cli::Command::Update { check_only }) => match install_cmd::update(check_only) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro update: {e}");
                ExitCode::FAILURE
            }
        },
        Ok(cli::Command::Install(args)) => match install_cmd::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bro install: {e}");
                ExitCode::FAILURE
            }
        },
        // a running bro has its own environment: with --env, open a new one that has these variables
        // `bro` opens the desktop app wherever there's a desktop to open it on; `bro-cli` the terminal UI
        #[cfg(feature = "gui")]
        Ok(cli::Command::Tui { demo, new, dir }) if gui.is_some() || (!tui && desktop()) => {
            ensure_bro_cli();
            match app::gui::launch(gui.unwrap_or_default(), demo, new || !env.sources.is_empty(), dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("bro: {e:#}\n(bro-cli opens the terminal UI)");
                    ExitCode::FAILURE
                }
            }
        }
        Ok(cli::Command::Tui { demo, new, dir }) => match {
            ensure_bro_cli();
            run_tui(demo, new || !env.sources.is_empty(), dir)
        } {
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
/// `--tui` / `--cli` (what `bro-cli` passes) or BRO_UI=tui|cli: the terminal UI rather than the desktop app.
fn take_tui_flag(args: &mut Vec<String>) -> bool {
    let mut tui = std::env::var("BRO_UI").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "tui" | "cli" | "terminal"));
    while let Some(i) = args.iter().position(|a| a == "--tui" || a == "--cli") {
        args.remove(i);
        tui = true;
    }
    tui
}

/// Logged in over SSH.
fn over_ssh() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    set("SSH_CONNECTION") || set("SSH_TTY")
}

/// A desktop to open a window on: not over SSH, not inside tmux (that's asking for the terminal UI), and on Linux
/// and the BSDs an X11 or Wayland display.
#[cfg(feature = "gui")]
fn desktop() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if over_ssh() || set("TMUX") {
        return false;
    }
    if cfg!(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly")) {
        return set("DISPLAY") || set("WAYLAND_DISPLAY");
    }
    true
}

/// `bro-cli` beside this bro: written when it's missing (an update from a release that didn't ship it brings only
/// bro itself). Not in a build folder.
fn ensure_bro_cli() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)) else { return };
    if dir.ends_with("debug") || dir.ends_with("release") || dir.ends_with("deps") {
        return;
    }
    #[cfg(windows)]
    let (name, body) = ("bro-cli.cmd", include_str!("../../../packaging/bro-cli.cmd"));
    #[cfg(not(windows))]
    let (name, body) = ("bro-cli", include_str!("../../../packaging/bro-cli"));
    let path = dir.join(name);
    if path.exists() || util::atomic_write(&path, body.as_bytes()).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
}

/// Set up the terminal, start services, run the app, restore the terminal.
fn run_tui(demo: bool, new: bool, dir: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    use crossterm::{event, execute, terminal};
    enable_vt_output();
    let named_folder = dir.is_some();
    // the folder bro was opened in (or `bro <folder>`) becomes the current project
    let dir = match dir {
        Some(d) => util::strip_verbatim(std::fs::canonicalize(&d).map_err(|_| anyhow::anyhow!("{} isn't a folder", d.display()))?),
        None => std::env::current_dir()?,
    };
    let mut viewer = services::host::role() == bro_host::Role::Viewer;
    // running inside tmux? show that bro here (an SSH login reaches the same sessions)
    if !demo && !new && !viewer {
        match instance::running_in_tmux() {
            Some(Some(pane)) => {
                if named_folder {
                    let _ = instance::hand_off(&dir);
                }
                return pane.attach();
            }
            // a desktop bro: opening the folder there would show nothing to someone over SSH, so watch its sessions
            // here instead (they're in the session server)
            Some(None) if over_ssh() => {
                if bro_host::client::status(&bro_host::client::default_dir()).is_err() {
                    eprintln!(
                        "bro is already running on this machine's desktop, outside tmux, so it can't be shown over SSH
                         (its sessions aren't in the session server: Settings › Session server › Keep sessions running).
                         Start it inside tmux to reach it from anywhere: tmux new -A -s bro bro
                         (bro --new opens a separate one here)"
                    );
                    return Ok(());
                }
                services::host::set_role(bro_host::Role::Viewer);
                viewer = true;
            }
            _ => {}
        }
    }
    // already running? hand the folder over and get out of the way
    if !demo && !new && !viewer && instance::hand_off(&dir).unwrap_or(false) {
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
    // a second bro beside a running one (`--new`) leaves the session server to that one
    if new && instance::running() && !viewer {
        services::host::solo();
    }
    // the answers to what sessions ask their terminal: this terminal UI's colours
    services::host::set_profile(services::host::tui_profile(&theme::get(&settings.theme)));
    let svc = match () {
        _ if demo => services::Services::demo(settings, tx.clone()),
        _ if viewer => services::Services::start_viewer(settings, tx.clone()),
        _ => services::Services::start(settings, tx.clone()),
    };
    if viewer && svc.host().is_none() {
        drop(svc);
        anyhow::bail!("there's no session server to watch (is bro running, with Keep sessions running on?)");
    }

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

    let (input_tx, nav_tx) = (tx.clone(), tx.clone());
    let console = paste_burst::Console::new(Box::new(move |raw| {
        let _ = nav_tx.send(match raw {
            paste_burst::Raw::Nav(back) => pane::Event::Navigate(back),
            paste_burst::Raw::Alt(down) => pane::Event::Alt(down),
        });
    }));
    std::thread::Builder::new().name("bro-input".into())
        .spawn(move || paste_burst::pump(console, |ev| {
            perf::input_read();
            input_tx.send(pane::Event::Input(ev)).is_ok()
        }))?;

    let _instance = if demo || viewer { None } else { instance::serve(tx.clone()) };
    let mut app = app::App::new(svc, tx, app::Opts { demo, fixed_demo: false, load_recents: !demo });
    if viewer {
        // the orchestrator, settings files and the like are the other bro's
        app.guest = true;
        app.persist = false;
    }
    if !demo {
        app.start_in(dir);
        // sessions the session server kept running while bro was closed (or, watching, the other bro's)
        app.reattach_sessions();
        // an orchestrator that was running when bro last stopped comes back the way it was launched
        if !viewer {
            app.restore_orchestrator();
        }
    }
    if let Some(p) = problem.filter(|p| !services::guard::is_unimplemented(p)) {
        app.toast(alerts::Kind::Error, format!("settings: {p} — using defaults"));
    }
    let res = app.run(&mut term, rx);
    services::guard::restore_terminal();
    let n = app.left_running;
    let (s, them) = if n == 1 { ("", "it") } else { ("s", "them") };
    if viewer {
        println!("stopped watching · the sessions carry on in your other bro");
    } else if app.taken_over {
        println!("bro opened in another window: it has your sessions now");
    } else if app.left_to_background {
        println!("{n} session{s} still running · bro carries on with {them} in the background (phone bridge, auto-resume, watchdogs) · run bro to pick {them} up again");
    } else if n > 0 {
        println!("{n} session{s} still running in bro's session server · run bro to pick {them} up again");
    }
    res
}

/// `bro background`: the background bro. It drives the session server while no window is open, running the
/// workspace with no screen (see [`app::App::run_headless`]); a window that opens takes over and it stops.
fn run_background() -> anyhow::Result<()> {
    services::host::set_role(bro_host::Role::Background);
    services::guard::install_panic_hook();
    util::probe_local_offset();
    let (settings, _) = services::load_settings();
    if !settings.server.keep_sessions {
        return Ok(());
    }
    if !settings.server.keep_working {
        // signed in with bro set to start then, but no background bro wanted: only the server
        if let Ok(exe) = std::env::current_exe() {
            let _ = bro_host::client::spawn_detached(&exe, &["host"], &bro_host::client::default_dir());
        }
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let svc = services::Services::start(settings, tx.clone());
    // a window drives it (or it couldn't start): nothing to stand in for
    if svc.host().is_none() {
        svc.shutdown();
        return Ok(());
    }
    let mut app = app::App::new(svc, tx, app::Opts { demo: false, fixed_demo: false, load_recents: true });
    app.reattach_sessions();
    app.restore_orchestrator();
    app.run_headless(rx);
    Ok(())
}

/// Make sure escape sequences (colours, sixel logos) are interpreted, not printed or dropped.
///
/// crossterm decides once whether to use ANSI or the legacy console API, by trying to switch the console
/// into VT mode. When bro is the first program in a fresh console window (the bro-app window, a bare conhost)
/// that can fail, and with no TERM set crossterm then falls back to the Win32 colour API — which has no RGB,
/// so every colour comes out grey. Switch VT mode on ourselves and set TERM so crossterm always picks ANSI.
fn enable_vt_output() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
            SetConsoleMode,
        };
        // SAFETY: plain console-mode calls on our own stdout handle.
        unsafe {
            let h = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(h, &mut mode) != 0 {
                let want = mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
                if SetConsoleMode(h, want | DISABLE_NEWLINE_AUTO_RETURN) == 0 {
                    SetConsoleMode(h, want);
                }
            }
        }
    }
    if std::env::var_os("TERM").is_none() {
        // SAFETY: called at the very start of run_tui, before any other thread exists.
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }
}
