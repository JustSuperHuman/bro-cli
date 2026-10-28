//! `bro skills`: the git repos bro watches for agent skills (see [`bro_core::skill_repos`]), from a shell.
//! The same list as the Skills view's (`[skills] repos` in `~/.bro/v2.toml`). Also every skill installed, by where it
//! came from and in a project ([`bro_core::skill_inventory`]), installing a watched repo's skills everywhere or into a
//! project, disabling and enabling skills ([`bro_core::disabled_skills`]), and the Skill Issue marketplace
//! ([`bro_core::skill_market`]) to search and install from, as the view does.
use bro_core::config::Settings;
use bro_core::disabled_skills;
use bro_core::skill_inventory::{self, Origin};
use bro_core::skill_market::{self, DOMAINS, Sort};
use bro_core::skill_repos::{self, Target};
use std::path::PathBuf;

pub const HELP: &str = "bro skills — agent skills from git repos bro watches, and the Skill Issue marketplace

usage
  bro skills                    the watched repos, each skill in them (✓ installed everywhere)
  bro skills installed [--project <dir>]
                                every skill your agents have, by where it came from (bro, a repo,
                                Skill Issue, Codex, yours) and the agents that get it; with a
                                project, its own skills too
  bro skills add <repo>         watch a repo (https://github.com/owner/repo, owner/repo or any
                                git URL): bro clones it and lists its skills; nothing is installed
                                until you install it
  bro skills add <repo> --skill <name>
                                watch only that skill of the repo (owner/repo#name works too)
  bro skills install <repo> <skill>… [--project <dir>]
  bro skills install <repo> --all [--project <dir>]
                                install skills of a watched repo (watched now if it isn't):
                                everywhere (every Claude and Codex home), or into a project's
                                .claude/skills and .agents/skills; bro keeps them in step with it
  bro skills remove <repo>      stop watching it: what you installed from it stays, no longer
                                updated
  bro skills sync [repo]        pull every watched repo (or one) and bring what you installed
                                from it up to date now

  bro skills disable <skill> [--project <dir>]
                                move a skill out of your agents' reach, every copy of it, into
                                ~/.bro/disabled-skills (bro's own are switched off in settings)
  bro skills enable <skill>     put a disabled skill back where it was
  bro skills disabled           the disabled skills
  bro skills delete <skill>     delete a disabled skill for good

  bro skills search [words] [--domain <d>] [--sort ranked|clicks|name] [--all]
                                search Skill Issue (skill-issue.in), ranked as the site ranks them;
                                domains: design frontend backend motion-graphics game-dev finance
                                research social-media automation-tools
  bro skills install <id|name> [--project <dir>]
                                install a skill from Skill Issue (bro watches it from its repo)

A skill is a folder holding a SKILL.md: the repo's root, a folder at its root, or a folder
under its skills/ (in a repo with none there: under .claude/skills/, .agents/skills/ or
plugins/*/skills/). A skill of yours with the same name is never replaced. In bro, the Skills
view (`skills` in the palette, or settings → Skills → Manage skills) shows them all, by source
and by project, watches repos and browses the marketplace.
";

/// What `bro skills` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsCmd {
    List,
    /// every skill installed; with a project, its own too
    Installed(Option<PathBuf>),
    Add(String),
    Remove(String),
    Sync(Option<String>),
    Search { query: String, domain: Option<String>, sort: Sort, all: bool },
    /// a skill from Skill Issue, by id or name
    Install(String, Option<PathBuf>),
    /// skills of a repo (all of them when `skills` is empty: `--all`, or a spec that picks one)
    InstallFrom { repo: String, skills: Vec<String>, project: Option<PathBuf> },
    Disable(String, Option<PathBuf>),
    Enable(String),
    Disabled,
    Delete(String),
}

/// `--project <dir>` (or `-p`) out of `args`, and the rest.
fn project_flag(args: &[String]) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let mut rest = vec![];
    let mut project = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--project" || a == "-p" {
            project = Some(PathBuf::from(it.next().ok_or("--project needs a folder")?));
        } else {
            rest.push(a.clone());
        }
    }
    Ok((rest, project))
}

/// None: print [`HELP`].
pub fn parse(args: &[String]) -> Result<Option<SkillsCmd>, String> {
    let repo = |a: Option<&String>| -> Result<String, String> {
        let a = a.ok_or("which repo? (owner/repo or a git URL)")?;
        skill_repos::normalize(a).ok_or_else(|| format!("'{a}' isn't a repo: use owner/repo, https://github.com/owner/repo or a git URL"))
    };
    match args.first().map(String::as_str) {
        None | Some("list" | "ls") if args.len() <= 1 => Ok(Some(SkillsCmd::List)),
        Some("-h" | "--help" | "help") => Ok(None),
        Some("installed" | "have") => match &args[1..] {
            [] => Ok(Some(SkillsCmd::Installed(None))),
            [flag, dir] if flag == "--project" || flag == "-p" => Ok(Some(SkillsCmd::Installed(Some(PathBuf::from(dir))))),
            [dir] if !dir.starts_with('-') => Ok(Some(SkillsCmd::Installed(Some(PathBuf::from(dir))))),
            _ => Err("bro skills installed [--project <dir>]".into()),
        },
        Some("add" | "watch") if args.len() == 2 => repo(args.get(1)).map(|r| Some(SkillsCmd::Add(r))),
        Some("add" | "watch") if args.len() == 4 && matches!(args[2].as_str(), "--skill" | "-s") => {
            let base = repo(args.get(1))?;
            skill_repos::normalize(&format!("{base}#{}", args[3])).map(|r| Some(SkillsCmd::Add(r))).ok_or_else(|| format!("'{}' isn't a skill name or folder", args[3]))
        }
        Some("remove" | "rm" | "unwatch") if args.len() == 2 => repo(args.get(1)).map(|r| Some(SkillsCmd::Remove(r))),
        Some("sync" | "pull") if args.len() <= 2 => args.get(1).map(|a| repo(Some(a))).transpose().map(|r| Some(SkillsCmd::Sync(r))),
        Some("search" | "find" | "browse") => {
            let (mut words, mut domain, mut sort, mut all) = (vec![], None, Sort::Ranked, false);
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--domain" | "-d" => {
                        let d = it.next().ok_or("--domain needs one of: design frontend backend motion-graphics game-dev finance research social-media automation-tools")?;
                        let slug = DOMAINS.iter().find(|(s, l)| s.eq_ignore_ascii_case(d) || l.eq_ignore_ascii_case(d)).map(|(s, _)| s.to_string());
                        domain = Some(slug.ok_or_else(|| format!("no domain '{d}' (bro skills --help lists them)"))?);
                    }
                    "--sort" => {
                        sort = match it.next().map(String::as_str) {
                            Some("ranked" | "rank") => Sort::Ranked,
                            Some("clicks" | "popular") => Sort::Clicks,
                            Some("name" | "az") => Sort::Name,
                            other => return Err(format!("--sort takes ranked, clicks or name, not {}", other.unwrap_or("nothing"))),
                        }
                    }
                    "--all" | "-a" => all = true,
                    w => words.push(w.to_string()),
                }
            }
            Ok(Some(SkillsCmd::Search { query: words.join(" "), domain, sort, all }))
        }
        Some("install" | "get") if args.len() >= 2 => {
            let (rest, project) = project_flag(&args[1..])?;
            let (all, names): (Vec<String>, Vec<String>) = rest[1..].iter().cloned().partition(|a| a == "--all" || a == "-a");
            match (skill_repos::normalize(&rest[0]), &names[..]) {
                // a repo (or one skill of it): name its skills, or --all
                (Some(r), []) if all.is_empty() && skill_repos::pick(&r).is_none() => Err(format!("which of {r}'s skills? name them, or --all (bro skills lists what it holds)")),
                (Some(r), _) => Ok(Some(SkillsCmd::InstallFrom { repo: r, skills: names, project })),
                (None, []) if all.is_empty() => Ok(Some(SkillsCmd::Install(rest[0].clone(), project))),
                (None, _) => Err(format!("'{}' isn't a repo: bro skills install <owner/repo> <skill>…, or a Skill Issue id", rest[0])),
            }
        }
        Some("disable" | "off") if args.len() >= 2 => {
            let (rest, project) = project_flag(&args[1..])?;
            match &rest[..] {
                [name] => Ok(Some(SkillsCmd::Disable(name.clone(), project))),
                _ => Err("bro skills disable <skill> [--project <dir>]".into()),
            }
        }
        Some("enable" | "on") if args.len() == 2 => Ok(Some(SkillsCmd::Enable(args[1].clone()))),
        Some("disabled") if args.len() == 1 => Ok(Some(SkillsCmd::Disabled)),
        Some("delete") if args.len() == 2 => Ok(Some(SkillsCmd::Delete(args[1].clone()))),
        Some(other) => Err(format!("unknown arguments '{other} …' (bro skills --help)")),
        None => Ok(None),
    }
}

/// The projects a sync brings up to date too, and installs and disables look in: the sidebar's.
fn project_roots() -> Vec<PathBuf> {
    crate::projects::OpenProjects::load().roots
}

/// A folder given on the command line, as the inventory knows it (whole, without Windows' `\\?\`).
fn folder(dir: &PathBuf) -> Result<PathBuf, String> {
    std::fs::canonicalize(dir).map(|p| bro_core::util::normalize_path(&p)).map_err(|e| format!("{}: {e}", dir.display()))
}

fn print_repo(repo: &str, skills: &skill_repos::RepoSkills) {
    let commit = skills.commit.as_deref().map(|c| format!(" @ {c}")).unwrap_or_default();
    println!("{}{commit}", skill_repos::web_url(repo));
    if skills.skills.is_empty() {
        println!("  no skills found");
    }
    let homes: Vec<PathBuf> = bro_core::justimagine::skill_homes().into_iter().map(|h| h.join("skills")).collect();
    let installed = skill_repos::installed_names(repo, &homes);
    for s in &skills.found {
        let mark = if installed.contains(&s.name) { "✓" } else { " " };
        println!("  {mark} {:<28} {}", s.name, short(s.description.as_deref()));
    }
    if !skills.taken.is_empty() {
        println!("  held by a skill of yours of the same name: {}", skills.taken.join(", "));
    }
}

/// Where a skill came from, as `bro skills installed` heads its group.
fn origin_head(o: &Origin) -> String {
    match o {
        Origin::Bro => "bundled with bro".into(),
        Origin::Repo(spec) if skill_repos::pick(spec).is_some() => format!("from Skill Issue ({})", skill_repos::label(spec)),
        Origin::Repo(spec) => format!("from {}", skill_repos::web_url(spec)),
        Origin::Codex => "built into Codex".into(),
        Origin::Yours => "yours (bro leaves them alone)".into(),
    }
}

/// A description cut to fit a line.
fn short(what: Option<&str>) -> String {
    let what = what.map(skill_inventory::one_line).unwrap_or_default();
    if what.chars().count() > 70 { what.chars().take(69).chain(['…']).collect() } else { what }
}

/// Every skill installed, grouped by where it came from; then the project's own (or each open project's).
fn installed(project: Option<PathBuf>) -> Result<(), String> {
    let roots = match &project {
        Some(dir) => vec![folder(dir)?],
        None => project_roots(),
    };
    let inv = skill_inventory::scan(&roots);
    let mut head: Option<(Option<PathBuf>, Origin)> = None;
    let show = |s: &skill_inventory::Skill| {
        let agents: Vec<&str> = s.agents().into_iter().map(|a| a.label()).collect();
        println!("  {:<28} {:<20} {}", s.name, agents.join(" "), short(s.description.as_deref()));
    };
    for s in &inv.skills {
        let key = (s.project.clone(), s.origin.clone());
        if head.as_ref() != Some(&key) {
            match &s.project {
                Some(root) => println!("\nin {} · {}", root.display(), origin_head(&s.origin)),
                None => println!("\n{}", origin_head(&s.origin)),
            }
            head = Some(key);
        }
        show(s);
    }
    if let Some(root) = roots.first().filter(|_| project.is_some())
        && inv.of_project(root).next().is_none()
    {
        println!("\n{} has no skills of its own (.claude/skills, .codex/skills, .agents/skills)", root.display());
    }
    let everywhere = inv.everywhere().count();
    println!("\n{} in every project{}", if everywhere == 1 { "1 skill".to_string() } else { format!("{everywhere} skills") }, if inv.skills.len() > everywhere { format!(" · {} more in projects", inv.skills.len() - everywhere) } else { String::new() });
    if !inv.disabled.is_empty() {
        println!("{} disabled · bro skills disabled", inv.disabled.len());
    }
    Ok(())
}

fn sync(repo: &str) -> Result<(), String> {
    let skills = skill_repos::sync(repo, &project_roots()).map_err(|e| format!("{}: {e}", skill_repos::label(repo)))?;
    print_repo(repo, &skills);
    Ok(())
}

/// Watch `repo` (pulling it first) and save it in the settings.
fn add(repo: String) -> Result<(), String> {
    let mut s = Settings::load();
    let watched = s.skills.repos.iter().any(|r| skill_repos::same(r, &repo));
    if watched {
        println!("already watching {} · pulling it again", skill_repos::label(&repo));
    }
    sync(&repo)?;
    if !watched {
        s.skills.repos.push(repo.clone());
        s.save().map_err(|e| format!("couldn't save ~/.bro/v2.toml: {e:#}"))?;
    }
    println!("install the ones you want: bro skills install {} <skill>… (or --all) · or in bro's Skills view", skill_repos::base(&repo));
    Ok(())
}

/// Install `skills` of `repo` (every one when empty) everywhere or into `project`, watching it first when it isn't.
fn install_from(repo: String, skills: Vec<String>, project: Option<PathBuf>) -> Result<(), String> {
    let target = match &project {
        Some(dir) => Target::Project(folder(dir)?),
        None => Target::Everywhere,
    };
    let mut s = Settings::load();
    if !s.skills.repos.iter().any(|r| skill_repos::same(r, &repo)) {
        s.skills.repos.push(repo.clone());
        s.save().map_err(|e| format!("couldn't save ~/.bro/v2.toml: {e:#}"))?;
        println!("watching {}", skill_repos::label(&repo));
    }
    let done = skill_repos::install(&repo, &skills, &target)?;
    let place = match &target {
        Target::Everywhere => "everywhere (every Claude and Codex home)".to_string(),
        Target::Project(root) => format!("into {}", root.display()),
    };
    if !done.written.is_empty() {
        println!("installed {} {place} · new sessions get {}", done.written.join(", "), if done.written.len() == 1 { "it" } else { "them" });
    }
    if !done.taken.is_empty() {
        println!("left out (you have a skill of that name): {}", done.taken.join(", "));
    }
    Ok(())
}

/// The installed skill named `name` (in `project`'s own folders, else everywhere).
fn disable(name: &str, project: Option<PathBuf>) -> Result<(), String> {
    let root = project.as_ref().map(folder).transpose()?;
    let roots: Vec<PathBuf> = root.iter().cloned().collect();
    let inv = skill_inventory::scan(&roots);
    let here = |s: &&skill_inventory::Skill| match (&root, &s.project) {
        (Some(r), Some(p)) => bro_core::util::same_path(p, r),
        (None, None) => true,
        _ => false,
    };
    let found: Vec<&skill_inventory::Skill> = inv.skills.iter().filter(|s| s.name.eq_ignore_ascii_case(name)).filter(here).collect();
    let skill = match found[..] {
        [] => return Err(format!("no installed skill {name}{} (bro skills installed lists them)", root.as_ref().map(|r| format!(" in {}", r.display())).unwrap_or_default())),
        [one] => one,
        _ => return Err(format!("more than one skill is named {name}: {} · bro skills installed", found.iter().map(|s| origin_head(&s.origin)).collect::<Vec<_>>().join(", "))),
    };
    let d = disabled_skills::disable(skill)?;
    println!("disabled {} · kept in {} · bro skills enable {} puts it back", d.name, d.dir().display(), d.name);
    Ok(())
}

/// The disabled skill named (or kept as) `name`.
fn disabled_one(name: &str) -> Result<disabled_skills::Disabled, String> {
    let all = disabled_skills::list();
    let named: Vec<_> = all.iter().filter(|d| d.id.eq_ignore_ascii_case(name) || d.name.eq_ignore_ascii_case(name)).collect();
    match named[..] {
        [] => Err(format!("no disabled skill {name} (bro skills disabled lists them)")),
        [one] => Ok(one.clone()),
        _ => Err(format!("more than one disabled skill is named {name}: {} · use one of those", named.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

pub fn run(cmd: SkillsCmd) -> Result<(), String> {
    let mut s = Settings::load();
    match cmd {
        SkillsCmd::List => {
            if s.skills.repos.is_empty() {
                println!("no repos watched · bro skills add <owner/repo> · bro skills search <words>");
            }
            for repo in &s.skills.repos {
                match skill_repos::cached(repo) {
                    Some(skills) => print_repo(repo, &skills),
                    None => println!("{}: not pulled yet · bro skills sync", skill_repos::web_url(repo)),
                }
            }
            let own = skill_repos::own_skills();
            if !own.is_empty() {
                println!("yours in ~/.claude/skills (bro leaves them alone): {}", own.join(", "));
            }
            Ok(())
        }
        SkillsCmd::Installed(project) => installed(project),
        SkillsCmd::Add(repo) => add(repo),
        SkillsCmd::Remove(repo) => {
            let before = s.skills.repos.len();
            s.skills.repos.retain(|r| !skill_repos::same(r, &repo));
            if s.skills.repos.len() != before {
                s.save().map_err(|e| format!("couldn't save ~/.bro/v2.toml: {e:#}"))?;
            }
            skill_repos::forget(&repo);
            println!("stopped watching {} · what you installed from it stays, no longer updated (bro skills disable <skill> takes one out)", skill_repos::label(&repo));
            Ok(())
        }
        SkillsCmd::Sync(one) => {
            let repos = match one {
                Some(r) => vec![r],
                None if s.skills.repos.is_empty() => return Err("no repos watched · bro skills add <owner/repo>".into()),
                None => s.skills.repos.clone(),
            };
            let failed: Vec<String> = repos.iter().filter_map(|r| sync(r).err()).collect();
            if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
        }
        SkillsCmd::Search { query, domain, sort, all } => {
            let cat = skill_market::load(false).map_err(|e| format!("couldn't fetch skill-issue.in: {e:#}"))?;
            let found = cat.list(domain.as_deref(), &query, sort);
            if found.is_empty() {
                println!("nothing matches");
                return Ok(());
            }
            let watched = &s.skills.repos;
            let shown = if all { found.len() } else { found.len().min(25) };
            for &i in &found[..shown] {
                let k = &cat.skills[i];
                let installed = skill_market::source(k).is_some_and(|spec| watched.iter().any(|w| skill_repos::same(w, &spec) || skill_repos::same(w, skill_repos::base(&spec))));
                let pop = k.popularity().unwrap_or_default();
                println!("{} {:<34} {:<14} {:<16} {}", if installed { "✓" } else { " " }, k.name, pop, k.domain, k.id);
                println!("    {}", k.use_case);
            }
            if shown < found.len() {
                println!("… {} more (--all lists them)", found.len() - shown);
            }
            println!("install one: bro skills install <id>");
            Ok(())
        }
        SkillsCmd::Install(what, project) => {
            let cat = skill_market::load(false).map_err(|e| format!("couldn't fetch skill-issue.in: {e:#}"))?;
            let w = what.to_lowercase();
            let k = cat.skills.iter().find(|k| k.id.to_lowercase() == w).or_else(|| {
                let named: Vec<_> = cat.skills.iter().filter(|k| k.name.to_lowercase() == w).collect();
                (named.len() == 1).then(|| named[0])
            });
            let k = k.ok_or_else(|| format!("no skill with the id or name '{what}' on skill-issue.in (bro skills search {what})"))?;
            let spec = skill_market::source(k).ok_or_else(|| format!("{}: bro can't tell which repo it's in · see {}", k.name, k.link))?;
            println!("{} from {}", k.name, skill_repos::label(&spec));
            install_from(spec, vec![], project)
        }
        SkillsCmd::InstallFrom { repo, skills, project } => install_from(repo, skills, project),
        SkillsCmd::Disable(name, project) => disable(&name, project),
        SkillsCmd::Enable(name) => {
            let d = disabled_one(&name)?;
            disabled_skills::enable(&d)?;
            println!("enabled {} · new sessions get it again", d.name);
            Ok(())
        }
        SkillsCmd::Disabled => {
            let all = disabled_skills::list();
            if all.is_empty() {
                println!("nothing disabled · bro skills disable <skill> moves one out of your agents' reach");
            }
            for d in &all {
                let place = d.project.as_ref().map(|p| format!("in {}", p.display())).unwrap_or_else(|| "everywhere".into());
                let agents: Vec<&str> = d.agents().into_iter().map(|a| a.label()).collect();
                println!("  {:<28} {:<14} {:<20} {}", d.id, place, agents.join(" "), short(d.description.as_deref()));
            }
            if !all.is_empty() {
                println!("kept in {} · bro skills enable <skill> puts one back", disabled_skills::root().display());
            }
            Ok(())
        }
        SkillsCmd::Delete(name) => {
            let d = disabled_one(&name)?;
            disabled_skills::delete(&d)?;
            println!("deleted {} for good", d.name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Option<SkillsCmd>, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn parses_each_form() {
        assert_eq!(p(&[]), Ok(Some(SkillsCmd::List)));
        assert_eq!(p(&["list"]), Ok(Some(SkillsCmd::List)));
        assert_eq!(p(&["--help"]), Ok(None));
        assert_eq!(p(&["add", "https://github.com/t3dotgg/tstack"]), Ok(Some(SkillsCmd::Add("t3dotgg/tstack".into()))));
        assert_eq!(p(&["add", "wshobson/agents", "--skill", "react-state-management"]), Ok(Some(SkillsCmd::Add("wshobson/agents#react-state-management".into()))));
        assert_eq!(p(&["add", "wshobson/agents#react-state-management"]), Ok(Some(SkillsCmd::Add("wshobson/agents#react-state-management".into()))));
        assert_eq!(p(&["rm", "t3dotgg/tstack"]), Ok(Some(SkillsCmd::Remove("t3dotgg/tstack".into()))));
        assert_eq!(p(&["sync"]), Ok(Some(SkillsCmd::Sync(None))));
        assert_eq!(p(&["sync", "t3dotgg/tstack"]), Ok(Some(SkillsCmd::Sync(Some("t3dotgg/tstack".into())))));
        assert!(p(&["add"]).is_err() && p(&["add", "nope"]).is_err() && p(&["frobnicate"]).is_err());
        assert!(p(&["add", "o/r", "--skill", "../x"]).is_err());
        assert_eq!(p(&["installed"]), Ok(Some(SkillsCmd::Installed(None))));
        assert_eq!(p(&["installed", "--project", "/code/app"]), Ok(Some(SkillsCmd::Installed(Some(PathBuf::from("/code/app"))))));
        assert_eq!(p(&["installed", "."]), Ok(Some(SkillsCmd::Installed(Some(PathBuf::from("."))))));
        assert!(p(&["installed", "--nope", "x"]).is_err());
    }

    #[test]
    fn parses_installs_and_disabling() {
        // a repo's skills, by name or all of them, everywhere or into a project
        assert_eq!(p(&["install", "t3dotgg/tstack", "council", "sol-loop"]), Ok(Some(SkillsCmd::InstallFrom { repo: "t3dotgg/tstack".into(), skills: vec!["council".into(), "sol-loop".into()], project: None })));
        assert_eq!(p(&["install", "t3dotgg/tstack", "--all", "--project", "/code/app"]), Ok(Some(SkillsCmd::InstallFrom { repo: "t3dotgg/tstack".into(), skills: vec![], project: Some(PathBuf::from("/code/app")) })));
        assert_eq!(p(&["install", "wshobson/agents#react-state"]), Ok(Some(SkillsCmd::InstallFrom { repo: "wshobson/agents#react-state".into(), skills: vec![], project: None })));
        assert!(p(&["install", "t3dotgg/tstack"]).unwrap_err().contains("--all"));
        // a Skill Issue id
        assert_eq!(p(&["install", "design-taste-skill", "-p", "."]), Ok(Some(SkillsCmd::Install("design-taste-skill".into(), Some(PathBuf::from("."))))));
        assert!(p(&["install", "not-a-repo", "council"]).is_err() && p(&["install", "x", "--project"]).is_err());
        assert_eq!(p(&["disable", "council"]), Ok(Some(SkillsCmd::Disable("council".into(), None))));
        assert_eq!(p(&["disable", "release", "--project", "/code/app"]), Ok(Some(SkillsCmd::Disable("release".into(), Some(PathBuf::from("/code/app"))))));
        assert_eq!(p(&["enable", "council"]), Ok(Some(SkillsCmd::Enable("council".into()))));
        assert_eq!((p(&["disabled"]), p(&["delete", "council"])), (Ok(Some(SkillsCmd::Disabled)), Ok(Some(SkillsCmd::Delete("council".into())))));
        assert!(p(&["disable"]).is_err() && p(&["disable", "a", "b"]).is_err());
    }

    #[test]
    fn parses_search_and_install() {
        assert_eq!(p(&["search"]), Ok(Some(SkillsCmd::Search { query: String::new(), domain: None, sort: Sort::Ranked, all: false })));
        assert_eq!(
            p(&["search", "react", "state", "--domain", "Frontend", "--sort", "clicks", "--all"]),
            Ok(Some(SkillsCmd::Search { query: "react state".into(), domain: Some("frontend".into()), sort: Sort::Clicks, all: true }))
        );
        assert_eq!(p(&["search", "-d", "game-dev"]).unwrap(), Some(SkillsCmd::Search { query: String::new(), domain: Some("game-dev".into()), sort: Sort::Ranked, all: false }));
        assert!(p(&["search", "--domain", "cooking"]).is_err() && p(&["search", "--sort", "vibes"]).is_err());
        assert_eq!(p(&["install", "design-taste-skill"]), Ok(Some(SkillsCmd::Install("design-taste-skill".into(), None))));
    }
}
