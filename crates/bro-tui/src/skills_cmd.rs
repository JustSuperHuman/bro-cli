//! `bro skills`: the git repos bro watches for agent skills (see [`bro_core::skill_repos`]), from a shell.
//! The same list as the Skills section of settings (`[skills] repos` in `~/.bro/v2.toml`).
use bro_core::config::Settings;
use bro_core::skill_repos;

pub const HELP: &str = "bro skills — agent skills from git repos bro watches

usage
  bro skills                    the watched repos and the skills each brought
  bro skills add <repo>         watch a repo (https://github.com/owner/repo, owner/repo or any
                                git URL): its skills go into every Claude and Codex home now, and
                                stay in step with it
  bro skills remove <repo>      stop watching it and take its skills back out
  bro skills sync [repo]        pull every watched repo (or one) and refresh its skills now

A skill is a folder holding a SKILL.md: the repo's root, a folder at its root, or a folder
under its skills/. A skill of yours with the same name is never replaced.
";

/// What `bro skills` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsCmd {
    List,
    Add(String),
    Remove(String),
    Sync(Option<String>),
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
        Some("add" | "watch") if args.len() == 2 => repo(args.get(1)).map(|r| Some(SkillsCmd::Add(r))),
        Some("remove" | "rm" | "unwatch") if args.len() == 2 => repo(args.get(1)).map(|r| Some(SkillsCmd::Remove(r))),
        Some("sync" | "pull") if args.len() <= 2 => args.get(1).map(|a| repo(Some(a))).transpose().map(|r| Some(SkillsCmd::Sync(r))),
        Some(other) => Err(format!("unknown arguments '{other} …' (bro skills --help)")),
        None => Ok(None),
    }
}

fn print_repo(repo: &str, skills: &skill_repos::RepoSkills) {
    let commit = skills.commit.as_deref().map(|c| format!(" @ {c}")).unwrap_or_default();
    println!("{}{commit}: {}", skill_repos::web_url(repo), if skills.skills.is_empty() { "no skills found".into() } else { skills.skills.join(", ") });
    if !skills.taken.is_empty() {
        println!("  left out (you have your own of that name): {}", skills.taken.join(", "));
    }
}

fn sync(repo: &str) -> Result<(), String> {
    let skills = skill_repos::sync(repo).map_err(|e| format!("{repo}: {e}"))?;
    print_repo(repo, &skills);
    Ok(())
}

pub fn run(cmd: SkillsCmd) -> Result<(), String> {
    let mut s = Settings::load();
    match cmd {
        SkillsCmd::List => {
            if s.skills.repos.is_empty() {
                println!("no repos watched · bro skills add <owner/repo>");
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
        SkillsCmd::Add(repo) => {
            if s.skills.repos.contains(&repo) {
                println!("already watching {repo} · pulling it again");
            }
            sync(&repo)?;
            if !s.skills.repos.contains(&repo) {
                s.skills.repos.push(repo);
                s.save().map_err(|e| format!("couldn't save ~/.bro/v2.toml: {e:#}"))?;
            }
            println!("new Claude and Codex sessions get them; a running bro picks up the repo when it next opens settings");
            Ok(())
        }
        SkillsCmd::Remove(repo) => {
            let before = s.skills.repos.len();
            s.skills.repos.retain(|r| *r != repo);
            if s.skills.repos.len() != before {
                s.save().map_err(|e| format!("couldn't save ~/.bro/v2.toml: {e:#}"))?;
            }
            skill_repos::forget(&repo);
            println!("stopped watching {repo} and took its skills back out");
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
        assert_eq!(p(&["rm", "t3dotgg/tstack"]), Ok(Some(SkillsCmd::Remove("t3dotgg/tstack".into()))));
        assert_eq!(p(&["sync"]), Ok(Some(SkillsCmd::Sync(None))));
        assert_eq!(p(&["sync", "t3dotgg/tstack"]), Ok(Some(SkillsCmd::Sync(Some("t3dotgg/tstack".into())))));
        assert!(p(&["add"]).is_err() && p(&["add", "nope"]).is_err() && p(&["frobnicate"]).is_err());
    }
}
