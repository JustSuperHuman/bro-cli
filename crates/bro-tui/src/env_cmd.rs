//! `--env <file|url>` and `--save` (before any command), and `bro env encrypt | decrypt | list`.
//!
//! `--env` loads a dotenv file, from disk or a URL, into bro's own environment before anything
//! else starts, so every session and installer bro runs inherits it. An encrypted file asks for
//! its password (or reads `BRO_ENV_PASSWORD`). `--save` also writes the variables to the user's
//! persistent environment and restarts JustImagine's service so it picks up its keys.

use bro_core::envfile::{self, Vars};
use std::io::{IsTerminal, Write};

pub const HELP: &str = "bro env — env files: API keys and settings for bro, its sessions and JustImagine

usage
  bro --env <file|url> [command]   load the file into bro (and everything it starts), then run
                                   the command (none: open the workspace in a new window)
  bro --env <file|url> --save      also save the variables for every new program and service:
                                   Windows: your user environment variables; macOS / Linux:
                                   ~/.config/bro/env from your shell profiles, plus launchd /
                                   systemd. JustImagine's service restarts to pick them up
  bro env encrypt <file> [-o out]  write an encrypted copy (asks for a password twice);
                                   default out: <file>.enc
  bro env decrypt <file|url> [-o out]
                                   print the plain file (or write it to out)
  bro env list <file|url>          the variables it sets, values hidden
  bro --export-env [file] [--all] [--plain] [--force]
  bro env export [file] …          your own variables as an encrypted env file, bro.env by
                                   default (asks for a passphrase twice). Windows: your user
                                   environment variables; macOS / Linux: this shell's
                                   environment. OS, login and terminal variables are left out
                                   (--all keeps them). --plain skips the encryption; file -
                                   prints it; --force replaces an existing file

--env can be given more than once; a later file wins. URLs must be https (http only for
localhost). A URL without a file name (https://m.justgains.com, …/team/) loads bro.env from
there, then bro.env.enc. An encrypted file asks for its password, or takes it from BRO_ENV_PASSWORD.
PATH, HOME and other OS variables in a file are ignored.

file format
  OPENROUTER_API_KEY=sk-or-v1-…      # comments, `export KEY=…`, 'single' and \"double\" quotes
  ELEVENLABS_API_KEY=…
";

/// `--env` / `--save`, taken off the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvFlags {
    pub sources: Vec<String>,
    pub save: bool,
}

/// Removes `--env <x>`, `--env=<x>` and `--save` from `args`. They go before the command, so
/// scanning stops at the first word that isn't one of them or a flag (`bro proxy … --save` is
/// the proxy's business).
pub fn take_flags(args: &mut Vec<String>) -> Result<EnvFlags, String> {
    let mut flags = EnvFlags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--env=") {
            flags.sources.push(v.to_string());
            args.remove(i);
        } else if a == "--env" {
            args.remove(i);
            if i >= args.len() || args[i].starts_with('-') {
                return Err("--env needs a file or URL".into());
            }
            flags.sources.push(args.remove(i));
        } else if a == "--save" {
            flags.save = true;
            args.remove(i);
        } else if a.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    if flags.save && flags.sources.is_empty() {
        return Err("--save needs --env <file|url>".into());
    }
    Ok(flags)
}

/// A password typed without echo. `BRO_ENV_PASSWORD` answers instead when it's set.
fn ask_password(prompt: &str) -> Result<String, String> {
    if let Ok(p) = std::env::var(envfile::PASSWORD_VAR) {
        return Ok(p);
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!("no terminal to ask for the password: set {}", envfile::PASSWORD_VAR));
    }
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    crossterm::terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    let mut typed = String::new();
    let result = loop {
        match event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c' | 'd') if k.modifiers.contains(KeyModifiers::CONTROL) => break Err("cancelled".to_string()),
                KeyCode::Esc => break Err("cancelled".to_string()),
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) => typed.push(c),
                _ => {}
            },
            Ok(Event::Paste(text)) => typed.push_str(text.trim_end_matches(['\r', '\n'])),
            Ok(_) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result.map(|()| typed)
}

/// The plain text of `source`, asking for the password when it's encrypted (three tries).
fn read_plain(source: &str) -> Result<String, String> {
    let text = envfile::read_source(source).map_err(|e| format!("{e:#}"))?;
    if !envfile::is_encrypted(&text) {
        return Ok(text);
    }
    let from_var = std::env::var_os(envfile::PASSWORD_VAR).is_some();
    let mut last = String::new();
    for _ in 0..if from_var { 1 } else { 3 } {
        let pw = ask_password(&format!("password for {source}: "))?;
        match envfile::decrypt(&text, &pw) {
            Ok(plain) => return Ok(plain),
            Err(e) => {
                last = format!("{e:#}");
                if !from_var {
                    eprintln!("{last}");
                }
            }
        }
    }
    Err(format!("{source}: {last}"))
}

/// Every `--env` source, merged in order. Reserved names are dropped with a note.
pub fn load(flags: &EnvFlags) -> Result<Vars, String> {
    let mut all = Vars::new();
    for source in &flags.sources {
        let vars = envfile::parse(&read_plain(source)?).map_err(|e| format!("{source}: {e:#}"))?;
        for (k, v) in vars {
            envfile::upsert(&mut all, &k, v);
        }
    }
    let (vars, skipped) = envfile::usable(all);
    if !skipped.is_empty() {
        eprintln!("bro: ignored {} from the env file (the OS sets those)", skipped.join(", "));
    }
    Ok(vars)
}

/// Sets `vars` in bro's own environment.
///
/// # Safety
/// Call only while bro is single-threaded (first thing in `main`).
pub unsafe fn apply(vars: &Vars) {
    for (k, v) in vars {
        // SAFETY: the caller guarantees no other thread reads the environment yet.
        unsafe { std::env::set_var(k, v) };
    }
}

fn names(vars: &Vars) -> String {
    vars.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", ")
}

/// `--save`: the user's persistent environment, then JustImagine's service.
pub fn save(vars: &Vars) -> Result<(), String> {
    if vars.is_empty() {
        return Err("the env file sets nothing to save".into());
    }
    let places = envfile::save(vars).map_err(|e| format!("{e:#}"))?;
    eprintln!("saved {}:", names(vars));
    for p in &places {
        eprintln!("  {p}");
    }
    eprintln!("new terminals and programs get them; ones already open keep what they had");
    if let Some(found) = bro_core::justimagine::installed().filter(|f| f.dir.is_some()) {
        eprintln!("restarting JustImagine ({}) so it reads them…", found.describe());
        if let Err(e) = bro_core::justimagine::restart_service() {
            eprintln!("  couldn't restart it: {e} · `bro install justimagine --force` reinstalls and starts it");
        }
    }
    Ok(())
}

/// Load (and maybe save) what `flags` names, then say what was loaded. Single-threaded only.
pub fn run_flags(flags: &EnvFlags) -> Result<(), String> {
    let vars = load(flags)?;
    // SAFETY: main calls this before starting any thread.
    unsafe { apply(&vars) };
    if flags.save {
        save(&vars)?;
    } else {
        eprintln!("bro: loaded {} from {}", names(&vars), flags.sources.join(", "));
    }
    Ok(())
}

/// `bro env …`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvCmd {
    Encrypt { input: String, out: Option<String> },
    Decrypt { source: String, out: Option<String> },
    List { source: String },
    /// your own variables as an env file, encrypted unless `plain`; `out` `-` prints it
    Export { out: String, plain: bool, all: bool, force: bool },
}

/// `None` = print [`HELP`].
pub fn parse(args: &[String]) -> Result<Option<EnvCmd>, String> {
    let mut words = vec![];
    let mut out = None;
    let (mut plain, mut all, mut force) = (false, false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" | "help" => return Ok(None),
            "-o" | "--out" => out = Some(it.next().ok_or("-o needs a file")?.clone()),
            "--plain" => plain = true,
            "--all" | "-a" => all = true,
            "--force" | "-f" => force = true,
            "-" => words.push(a.clone()),
            s if s.starts_with('-') => return Err(format!("bro env: unknown option '{s}'")),
            _ => words.push(a.clone()),
        }
    }
    let one = |what: &str| -> Result<String, String> {
        match &words[1..] {
            [x] => Ok(x.clone()),
            [] => Err(format!("bro env {what}: name a file{}", if what == "encrypt" { "" } else { " or URL" })),
            _ => Err(format!("bro env {what}: one file at a time")),
        }
    };
    match words.first().map(String::as_str) {
        None => Ok(None),
        Some("encrypt") => Ok(Some(EnvCmd::Encrypt { input: one("encrypt")?, out })),
        Some("decrypt") => Ok(Some(EnvCmd::Decrypt { source: one("decrypt")?, out })),
        Some("list" | "ls") => Ok(Some(EnvCmd::List { source: one("list")? })),
        Some("export") => match &words[1..] {
            [] => Ok(Some(EnvCmd::Export { out: out.unwrap_or_else(|| envfile::DEFAULT_NAME.into()), plain, all, force })),
            [file] if out.is_none() => Ok(Some(EnvCmd::Export { out: file.clone(), plain, all, force })),
            _ => Err("bro env export: one file at most".into()),
        },
        Some(other) => Err(format!("bro env: unknown command '{other}' (encrypt, decrypt, list or export)")),
    }
}

fn write_out(path: &str, text: &str) -> Result<(), String> {
    bro_core::util::atomic_write(std::path::Path::new(path), text.as_bytes()).map_err(|e| format!("writing {path}: {e}"))
}

/// A new password, typed twice (or `BRO_ENV_PASSWORD`).
fn new_password() -> Result<String, String> {
    let pw = ask_password("new password: ")?;
    if std::env::var_os(envfile::PASSWORD_VAR).is_none() {
        if pw.chars().count() < 8 {
            return Err("use a password of at least 8 characters".into());
        }
        if ask_password("again: ")? != pw {
            return Err("the passwords don't match".into());
        }
    }
    Ok(pw)
}

pub fn run(cmd: EnvCmd) -> Result<(), String> {
    match cmd {
        EnvCmd::Encrypt { input, out } => {
            let text = std::fs::read_to_string(&input).map_err(|e| format!("reading {input}: {e}"))?;
            if envfile::is_encrypted(&text) {
                return Err(format!("{input} is already encrypted"));
            }
            let vars = envfile::parse(&text).map_err(|e| format!("{input}: {e:#}"))?;
            let pw = new_password()?;
            let armored = envfile::encrypt(&text, &pw).map_err(|e| format!("{e:#}"))?;
            let out = out.unwrap_or_else(|| format!("{input}.enc"));
            write_out(&out, &armored)?;
            println!("wrote {out} ({} variables: {})", vars.len(), names(&vars));
            println!("load it with: bro --env {out}   (add --save to keep them)");
            Ok(())
        }
        EnvCmd::Decrypt { source, out } => {
            let plain = read_plain(&source)?;
            match out {
                Some(path) => {
                    write_out(&path, &plain)?;
                    println!("wrote {path}");
                }
                None => print!("{plain}"),
            }
            Ok(())
        }
        EnvCmd::Export { out, plain, all, force } => {
            let to_stdout = out == "-";
            if !to_stdout && !force && std::path::Path::new(&out).exists() {
                return Err(format!("{out} already exists: --force replaces it, or name another file"));
            }
            let vars = envfile::export_filter(envfile::system_vars().map_err(|e| format!("{e:#}"))?, all);
            let header = format!("Exported by bro from {} on {}.\nLoad with: bro --env <this file>   (add --save to keep them)", envfile::system_source(), chrono::Local::now().format("%Y-%m-%d"));
            let mut text = envfile::to_dotenv(&vars, &header);
            if !plain {
                eprintln!("exporting {} variables: {}", vars.len(), names(&vars));
                text = envfile::encrypt(&text, &new_password()?).map_err(|e| format!("{e:#}"))?;
            }
            if to_stdout {
                print!("{text}");
                return Ok(());
            }
            envfile::write_private(std::path::Path::new(&out), &text).map_err(|e| format!("writing {out}: {e}"))?;
            println!("wrote {out}{} ({} variables: {})", if plain { "" } else { ", encrypted" }, vars.len(), names(&vars));
            println!("load it with: bro --env {out}   (add --save to keep them; host it as bro.env and bro --env <its folder URL> finds it)");
            Ok(())
        }
        EnvCmd::List { source } => {
            let vars = envfile::parse(&read_plain(&source)?).map_err(|e| format!("{source}: {e:#}"))?;
            for (k, v) in &vars {
                let n = v.chars().count();
                println!("{k:<32} {}", if n == 0 { "(empty)".to_string() } else { format!("{n} character{}", if n == 1 { "" } else { "s" }) });
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn takes_env_flags_before_the_command() {
        let mut a = s(&["--env", "keys.env", "--env=https://x/k.enc", "--new", "install", "justimagine", "--save"]);
        let f = take_flags(&mut a).unwrap();
        assert_eq!(f, EnvFlags { sources: s(&["keys.env", "https://x/k.enc"]), save: false });
        assert_eq!(a, s(&["--new", "install", "justimagine", "--save"]), "after the command, flags are the command's");
        let mut a = s(&["--env", "k.env", "--save"]);
        assert_eq!(take_flags(&mut a).unwrap(), EnvFlags { sources: s(&["k.env"]), save: true });
        assert!(a.is_empty());
        assert!(take_flags(&mut s(&["--save"])).is_err());
        assert!(take_flags(&mut s(&["--env"])).is_err() && take_flags(&mut s(&["--env", "--save"])).is_err());
        let mut a = s(&["F:/x"]);
        assert_eq!(take_flags(&mut a).unwrap(), EnvFlags::default());
        assert_eq!(a, s(&["F:/x"]));
    }

    #[test]
    fn parses_env_commands() {
        assert_eq!(parse(&s(&["encrypt", "keys.env"])), Ok(Some(EnvCmd::Encrypt { input: "keys.env".into(), out: None })));
        assert_eq!(parse(&s(&["encrypt", "keys.env", "-o", "k.enc"])), Ok(Some(EnvCmd::Encrypt { input: "keys.env".into(), out: Some("k.enc".into()) })));
        assert_eq!(parse(&s(&["decrypt", "https://x/k.enc"])), Ok(Some(EnvCmd::Decrypt { source: "https://x/k.enc".into(), out: None })));
        assert_eq!(parse(&s(&["list", "k.env"])), Ok(Some(EnvCmd::List { source: "k.env".into() })));
        assert_eq!(parse(&s(&["export"])), Ok(Some(EnvCmd::Export { out: "bro.env".into(), plain: false, all: false, force: false })));
        assert_eq!(parse(&s(&["export", "me.env", "--all", "--plain", "-f"])), Ok(Some(EnvCmd::Export { out: "me.env".into(), plain: true, all: true, force: true })));
        assert_eq!(parse(&s(&["export", "-"])), Ok(Some(EnvCmd::Export { out: "-".into(), plain: false, all: false, force: false })));
        assert!(parse(&s(&["export", "a", "b"])).is_err());
        assert_eq!(parse(&s(&[])), Ok(None));
        assert_eq!(parse(&s(&["--help"])), Ok(None));
        assert!(parse(&s(&["encrypt"])).is_err() && parse(&s(&["encrypt", "a", "b"])).is_err() && parse(&s(&["shred", "a"])).is_err());
    }

    #[test]
    fn encrypt_then_list_and_decrypt_with_the_password_variable() {
        crate::testkit::isolate();
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("keys.env");
        std::fs::write(&plain, "OPENROUTER_API_KEY=sk-or-v1-abc\nPATH=/evil\n").unwrap();
        // SAFETY: only this test reads the password variable.
        unsafe { std::env::set_var(envfile::PASSWORD_VAR, "correct horse battery") };
        let input = plain.display().to_string();
        run(EnvCmd::Encrypt { input: input.clone(), out: None }).unwrap();
        let enc = format!("{input}.enc");
        assert!(envfile::is_encrypted(&std::fs::read_to_string(&enc).unwrap()));
        let flags = EnvFlags { sources: vec![enc.clone()], save: false };
        assert_eq!(load(&flags).unwrap(), vec![("OPENROUTER_API_KEY".to_string(), "sk-or-v1-abc".to_string())], "PATH is dropped");
        let out = dir.path().join("back.env").display().to_string();
        run(EnvCmd::Decrypt { source: enc.clone(), out: Some(out.clone()) }).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), std::fs::read_to_string(&plain).unwrap());
        unsafe { std::env::set_var(envfile::PASSWORD_VAR, "wrong") };
        assert!(load(&flags).unwrap_err().contains("wrong password"));
        unsafe { std::env::remove_var(envfile::PASSWORD_VAR) };
    }
}
