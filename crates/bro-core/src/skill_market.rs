//! The Skill Issue marketplace (<https://skill-issue.in>): a curated, ranked catalogue of agent skills, each with
//! its domain, niche, creator, source link, popularity signal and `npx skills add` command, plus how often people
//! open it there. bro reads the catalogue out of the site's own page (the data its React app renders from), caches
//! it for half a day, and orders and filters it the way the site does: ranked by popularity, by domain.
//!
//! Installing a skill from it is watching it ([`crate::skill_repos`]): [`source`] turns an entry into the repo spec
//! bro watches, `owner/repo` for a collection or `owner/repo#skill` for one skill out of a bigger repo.
use crate::util::{atomic_write, env_str};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// The site (override `BRO_SKILL_MARKET_URL`).
pub const SITE: &str = "https://skill-issue.in/";
/// The cached catalogue is fresh for this long.
pub const CACHE_FOR: Duration = Duration::from_secs(12 * 3600);
const TIMEOUT: Duration = Duration::from_secs(30);

/// The domains, in the order the site's tabs run: (slug, label).
pub const DOMAINS: [(&str, &str); 9] = [
    ("design", "Design"),
    ("frontend", "Frontend"),
    ("backend", "Backend"),
    ("motion-graphics", "Motion Graphics"),
    ("game-dev", "Game Dev"),
    ("finance", "Finance"),
    ("research", "Research"),
    ("social-media", "Social Media"),
    ("automation-tools", "Automation Tools"),
];

/// A repo of skills bro features above the catalogue, to watch whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Featured {
    /// as `[skills] repos` keeps it
    pub repo: &'static str,
    pub title: &'static str,
    pub by: &'static str,
    pub blurb: &'static str,
    /// its skills, as its README lists them
    pub skills: &'static [&'static str],
    /// what some of them need
    pub needs: &'static str,
}

/// The repos Discover features: Theo's tstack.
pub const FEATURED: &[Featured] = &[Featured {
    repo: "t3dotgg/tstack",
    title: "tstack",
    by: "Theo · t3.gg",
    blurb: "The agent skills Theo uses every day: take a change all the way to a merged PR, get it reviewed by three models until they agree, then file, babysit and take over PRs.",
    skills: &["full-send", "full-stop", "council", "sol-loop", "file-pr", "takeover", "babysit-pr", "modernize", "leaving-pr-comments"],
    needs: "the GitHub CLI (gh), and Python 3 for babysit-pr",
}];

/// What a skill's popularity is counted in.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Metric {
    /// "stars", "installs" or "none"
    pub kind: String,
    pub value: Option<f64>,
}

/// One skill in the catalogue.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MarketSkill {
    pub id: String,
    pub name: String,
    pub domain: String,
    pub domain_slug: String,
    pub niche: Option<String>,
    pub use_case: String,
    pub creator: String,
    /// where it lives: a GitHub repo or folder, or its page on skills.sh / skillselion / officialskills.sh
    pub link: String,
    /// "90.4k stars", "862k installs", "Listed on skills.sh"
    pub signal: Option<String>,
    pub metric: Metric,
    /// someone opened the link and checked it by hand
    pub link_verified: bool,
    pub install_command: Option<String>,
}

impl MarketSkill {
    /// Every word of `query` is in its name, use, creator, niche or domain.
    pub fn matches(&self, query: &str) -> bool {
        let hay = format!("{} {} {} {} {}", self.name, self.use_case, self.creator, self.niche.as_deref().unwrap_or_default(), self.domain).to_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
    }

    /// How well it answers `query`: 3 its name is the query, 2 every word is in its name, 1 in its niche or
    /// creator, 0 only in what it does.
    pub fn relevance(&self, query: &str) -> u8 {
        let q = query.trim().to_lowercase();
        let name = self.name.to_lowercase();
        let in_all = |hay: &str| q.split_whitespace().all(|w| hay.contains(w));
        match () {
            _ if name == q || name == q.replace(' ', "-") => 3,
            _ if in_all(&name) => 2,
            _ if in_all(&format!("{} {}", self.niche.as_deref().unwrap_or_default(), self.creator).to_lowercase()) => 1,
            _ => 0,
        }
    }

    /// Its page on the site.
    pub fn page_url(&self) -> String {
        format!("{}/?skill={}", site().trim_end_matches('/'), self.id)
    }

    /// "90.4k stars", or nothing when the site has no number for it.
    pub fn popularity(&self) -> Option<String> {
        let v = self.metric.value.filter(|v| *v > 0.0)?;
        let what = match self.metric.kind.as_str() {
            "stars" => "stars",
            "installs" => "installs",
            _ => return None,
        };
        Some(format!("{} {what}", compact(v)))
    }
}

/// 90400 → "90.4k", 1_200_000 → "1.2M".
pub fn compact(v: f64) -> String {
    let trim = |s: String| s.strip_suffix(".0").map(str::to_string).unwrap_or(s);
    match v {
        v if v >= 1e6 => format!("{}M", trim(format!("{:.1}", v / 1e6))),
        v if v >= 1e3 => format!("{}k", trim(format!("{:.1}", v / 1e3))),
        v => format!("{v:.0}"),
    }
}

/// The whole catalogue.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Catalog {
    pub skills: Vec<MarketSkill>,
    /// skill id → how often people opened it on the site
    pub clicks: HashMap<String, u64>,
}

/// How the list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// the site's order: each skill's place among those counted the same way (stars, or installs), best first,
    /// then the ones with no number
    #[default]
    Ranked,
    /// opened most often on the site
    Clicks,
    /// by name
    Name,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::Ranked, Sort::Clicks, Sort::Name];
    pub fn label(self) -> &'static str {
        match self {
            Sort::Ranked => "ranked",
            Sort::Clicks => "most clicked",
            Sort::Name => "name",
        }
    }
    pub fn next(self) -> Sort {
        Sort::ALL[(Sort::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1) % Sort::ALL.len()]
    }
}

impl Catalog {
    pub fn clicks_of(&self, id: &str) -> u64 {
        self.clicks.get(id).copied().unwrap_or(0)
    }

    /// The skills in `domain` (None: all) that match `query`, as indexes into `skills`, ordered by `sort`; a
    /// search puts the skills it names first.
    pub fn list(&self, domain: Option<&str>, query: &str, sort: Sort) -> Vec<usize> {
        let q = query.trim();
        let mut out: Vec<usize> = self.order(sort).into_iter().filter(|&i| {
            let s = &self.skills[i];
            domain.is_none_or(|d| s.domain_slug == d) && (q.is_empty() || s.matches(q))
        }).collect();
        if !q.is_empty() {
            // stable: the sort holds within each tier
            out.sort_by_key(|&i| std::cmp::Reverse(self.skills[i].relevance(q)));
        }
        out
    }

    /// Every skill's index, ordered by `sort`.
    pub fn order(&self, sort: Sort) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.skills.len()).collect();
        match sort {
            Sort::Ranked => return ranked(&self.skills),
            Sort::Clicks => idx.sort_by_key(|&i| std::cmp::Reverse(self.clicks_of(&self.skills[i].id))),
            Sort::Name => idx.sort_by_key(|&i| self.skills[i].name.to_lowercase()),
        }
        idx
    }

    /// How many skills each domain holds.
    pub fn count(&self, domain: Option<&str>) -> usize {
        self.skills.iter().filter(|s| domain.is_none_or(|d| s.domain_slug == d)).count()
    }
}

/// The site's ranking: stars and installs aren't comparable, so each skill scores its place among those counted the
/// same way (1 the most, 0 the least), the two lists merge by that score, and the skills with no number follow in
/// the catalogue's order.
pub fn ranked(skills: &[MarketSkill]) -> Vec<usize> {
    let value = |i: usize| skills[i].metric.value.unwrap_or(0.0);
    let mut score: HashMap<usize, f64> = HashMap::new();
    let mut counted = vec![];
    for kind in ["stars", "installs"] {
        let mut group: Vec<usize> = (0..skills.len()).filter(|&i| skills[i].metric.kind == kind).collect();
        group.sort_by(|&a, &b| value(b).total_cmp(&value(a)));
        let n = group.len();
        for (place, &i) in group.iter().enumerate() {
            score.insert(i, if n > 1 { 1.0 - place as f64 / (n - 1) as f64 } else { 1.0 });
        }
        counted.extend((0..skills.len()).filter(|&i| skills[i].metric.kind == kind));
    }
    counted.sort_by(|a, b| score[b].total_cmp(&score[a]));
    counted.extend((0..skills.len()).filter(|i| !score.contains_key(i)));
    counted
}

/// Where the catalogue comes from.
pub fn site() -> String {
    env_str("BRO_SKILL_MARKET_URL").unwrap_or_else(|| SITE.into())
}

/// The catalogue in the site's page: its React Server Components payload (`self.__next_f.push([1, "…"])`
/// scripts) carries the props the list renders from, `"skills": [...]` and `"clickCounts": {...}`.
pub fn parse_page(html: &str) -> anyhow::Result<Catalog> {
    let mut payload = String::new();
    let mut rest = html;
    while let Some(at) = rest.find("self.__next_f.push(") {
        rest = &rest[at + "self.__next_f.push(".len()..];
        let mut it = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        if let Some(Ok(serde_json::Value::Array(chunk))) = it.next()
            && let Some(s) = chunk.get(1).and_then(|v| v.as_str())
        {
            payload.push_str(s);
        }
    }
    // a page without the payload (or an older copy of the data) is searched as it is
    let text = if payload.is_empty() { html } else { payload.as_str() };
    let skills: Vec<MarketSkill> = value_after(text, "\"skills\":").context("the page has no skill list (skill-issue.in may have changed)")?;
    let skills: Vec<MarketSkill> = skills.into_iter().filter(|s| !s.id.is_empty() && !s.name.is_empty()).collect();
    if skills.is_empty() {
        bail!("the page's skill list is empty");
    }
    let clicks = value_after(text, "\"clickCounts\":").unwrap_or_default();
    Ok(Catalog { skills, clicks })
}

/// The JSON value right after the first `key` in `text` that parses as a `T`.
fn value_after<T: serde::de::DeserializeOwned>(text: &str, key: &str) -> Option<T> {
    let mut from = 0;
    while let Some(at) = text[from..].find(key) {
        let start = from + at + key.len();
        let mut it = serde_json::Deserializer::from_str(&text[start..]).into_iter::<T>();
        if let Some(Ok(v)) = it.next() {
            return Some(v);
        }
        from = start;
    }
    None
}

fn cache_path() -> PathBuf {
    crate::paths::bro_dir().join("cache").join("skill-market.json")
}

/// The cached catalogue and how old it is; None when there's none.
pub fn cached() -> Option<(Catalog, Duration)> {
    let path = cache_path();
    let age = path.metadata().ok()?.modified().ok()?.elapsed().unwrap_or_default();
    let cat: Catalog = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (!cat.skills.is_empty()).then_some((cat, age))
}

/// The catalogue: the cache while it's fresh (unless `force`), else fetched (and cached). A failed fetch falls
/// back to an old cache. Blocking (network): call off the UI thread.
pub fn load(force: bool) -> anyhow::Result<Catalog> {
    let cache = cached();
    if !force && let Some((cat, age)) = &cache
        && *age < CACHE_FOR
    {
        return Ok(cat.clone());
    }
    let fetched = fetch().and_then(|cat| {
        let _ = atomic_write(&cache_path(), serde_json::to_string(&cat)?.as_bytes());
        Ok(cat)
    });
    match (fetched, cache) {
        (Ok(c), _) => Ok(c),
        (Err(_), Some((cat, _))) => Ok(cat),
        (Err(e), None) => Err(e),
    }
}

fn fetch() -> anyhow::Result<Catalog> {
    let url = site();
    let (status, html) = crate::http::get_text(&url, TIMEOUT)?;
    if !(200..300).contains(&status) {
        bail!("GET {url}: HTTP {status}");
    }
    parse_page(&html)
}

/// What bro watches to install `skill` ([`crate::skill_repos`]): `owner/repo#path` or `owner/repo#name` for one
/// skill in a repo, `owner/repo` when the entry is the whole repo. From its GitHub link when that points into the
/// repo, else its `npx skills add owner/repo --skill name`, else the owner/repo/skill its listing page is named
/// by. None when none of those say.
pub fn source(skill: &MarketSkill) -> Option<String> {
    let link = skill.link.trim();
    // https://github.com/owner/repo[/tree|blob/<ref>/<path>]
    let github = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://")).and_then(|l| l.strip_prefix("www.").unwrap_or(l).strip_prefix("github.com/"));
    let from_link = github.and_then(|path| {
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').filter(|p| !p.is_empty()).collect();
        let repo = crate::skill_repos::normalize(&format!("{}/{}", parts.first()?, parts.get(1)?))?;
        let inside = match parts.get(2) {
            Some(&("tree" | "blob")) if parts.len() > 4 => parts[4..].join("/"),
            _ => String::new(),
        };
        // a link to the SKILL.md (or another file in it) is its folder
        let inside = match inside.rsplit_once('/') {
            Some((dir, file)) if file.contains('.') => dir.to_string(),
            None if inside.contains('.') => String::new(),
            _ => inside,
        };
        Some((repo, inside))
    });
    // npx skills add <repo> [--skill <name>]
    let from_command = skill.install_command.as_deref().and_then(|c| {
        let words: Vec<&str> = c.split_whitespace().collect();
        let at = words.iter().position(|w| *w == "add")?;
        let repo = crate::skill_repos::normalize(words.get(at + 1)?)?;
        let name = words.iter().position(|w| *w == "--skill" || *w == "-s").and_then(|i| words.get(i + 1)).map(|s| s.to_string()).filter(|s| s != "SKILL.md" && !s.contains('.'));
        Some((repo, name))
    });
    // skills.sh, skillselion.com and officialskills.sh name a skill's page <owner>/<repo>/<skill>
    let from_listing = || {
        let path = link.split("://").nth(1)?;
        let (host, path) = path.split_once('/')?;
        let path = match host.trim_start_matches("www.") {
            "skillselion.com" => path.strip_prefix("skills/")?,
            "skills.sh" | "officialskills.sh" => path,
            _ => return None,
        };
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        let [owner, repo, name] = parts[..] else { return None };
        let repo = crate::skill_repos::normalize(&format!("{owner}/{repo}"))?;
        crate::skill_repos::normalize(&format!("{repo}#{name}"))
    };
    let spec = |repo: &str, inside: &str| if inside.is_empty() { Some(repo.to_string()) } else { crate::skill_repos::normalize(&format!("{repo}#{inside}")) };
    match (from_link, from_command) {
        // the link points into the repo: the exact folder
        (Some((repo, inside)), _) if !inside.is_empty() => spec(&repo, &inside),
        (_, Some((repo, Some(name)))) => spec(&repo, &name),
        (_, Some((repo, None))) => Some(repo),
        (Some((repo, _)), None) => Some(repo),
        (None, None) => from_listing(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(id: &str, kind: &str, value: Option<f64>) -> MarketSkill {
        MarketSkill { id: id.into(), name: id.into(), domain_slug: "design".into(), metric: Metric { kind: kind.into(), value }, ..Default::default() }
    }

    /// The page as Next.js serves it: the props inside a JSON string in a push script, among others.
    fn page(skills: &str, clicks: &str) -> String {
        let row = format!("6:[\"$\",\"$L1\",null,{{\"skills\":{skills},\"clickCounts\":{clicks}}}]\n");
        let quoted = serde_json::to_string(&row).unwrap();
        let half = quoted.len() / 2;
        // split mid-payload, as the site's chunks are, on a char boundary that isn't inside an escape
        let cut = (half..quoted.len()).find(|&i| quoted.is_char_boundary(i) && !quoted[..i].ends_with('\\')).unwrap();
        let (a, b) = (&quoted[..cut], &quoted[cut..]);
        format!("<html><script>self.__next_f.push([0])</script><script>self.__next_f.push([1,{a}\"])</script><script>self.__next_f.push([1,\"{b}])</script><a href=\"/?skill=x\">x</a></html>")
    }

    #[test]
    fn reads_the_catalogue_out_of_the_page() {
        let skills = r#"[{"id":"design-taste","name":"taste-skill","domain":"Design","domainSlug":"design","niche":"Design critique & taste","useCase":"Better \"frontend\" taste.","creator":"Leonxlnx","link":"https://github.com/Leonxlnx/taste-skill","signal":"41.4k stars","metric":{"kind":"stars","value":41400},"linkVerified":true,"linkType":"github","installCommand":"npx skills add Leonxlnx/taste-skill"},{"id":"x-none","name":"none","domain":"Finance","domainSlug":"finance","useCase":"u","creator":"c","link":"https://example.com","metric":{"kind":"none","value":null},"installCommand":null}]"#;
        let cat = parse_page(&page(skills, r#"{"design-taste":2670}"#)).unwrap();
        assert_eq!(cat.skills.len(), 2);
        assert_eq!(cat.skills[0].use_case, "Better \"frontend\" taste.");
        assert_eq!(cat.skills[0].popularity().as_deref(), Some("41.4k stars"));
        assert_eq!(cat.skills[1].install_command, None);
        assert_eq!(cat.clicks_of("design-taste"), 2670);
        assert!(parse_page("<html>nothing here</html>").is_err());
    }

    #[test]
    fn ranks_like_the_site() {
        let skills = vec![
            skill("s-low", "stars", Some(10.0)),
            skill("none", "none", None),
            skill("i-top", "installs", Some(900_000.0)),
            skill("s-top", "stars", Some(90_000.0)),
            skill("i-low", "installs", Some(5.0)),
            skill("s-mid", "stars", Some(500.0)),
        ];
        let names: Vec<&str> = ranked(&skills).into_iter().map(|i| skills[i].id.as_str()).collect();
        // each list's best scores 1, its worst 0; the one with no number comes last
        assert_eq!(names[..2].iter().copied().collect::<std::collections::HashSet<_>>(), ["s-top", "i-top"].into());
        assert_eq!(names[2], "s-mid");
        assert_eq!(names[5], "none");
        assert!(names[3..5].contains(&"s-low") && names[3..5].contains(&"i-low"));
    }

    #[test]
    fn filters_searches_and_sorts() {
        let mut a = skill("frontend-react", "stars", Some(100.0));
        a.name = "react-best-practices".into();
        a.domain_slug = "frontend".into();
        a.use_case = "React performance rules".into();
        let mut b = skill("design-ui", "stars", Some(1000.0));
        b.use_case = "UI patterns, including React components".into();
        let mut c = skill("design-zz", "installs", Some(3.0));
        c.name = "aardvark".into();
        let cat = Catalog { skills: vec![a, b, c], clicks: [("design-zz".to_string(), 50u64), ("design-ui".to_string(), 7)].into() };
        // stars and installs each rank their own: the best of each first, ties in catalogue order
        assert_eq!(cat.list(None, "", Sort::Ranked), [1, 2, 0]);
        assert_eq!(cat.list(Some("frontend"), "", Sort::Ranked), [0]);
        // "react" is in both; the one named for it comes first even though the other ranks higher
        assert_eq!(cat.list(None, "react", Sort::Ranked), [0, 1]);
        assert_eq!(cat.list(Some("design"), "", Sort::Clicks), [2, 1]);
        assert_eq!(cat.list(None, "", Sort::Name)[0], 2);
        assert_eq!(cat.count(Some("design")), 2);
        assert_eq!(compact(90_400.0), "90.4k");
        assert_eq!(compact(1_000_000.0), "1M");
    }

    #[test]
    fn finds_what_to_watch() {
        let with = |link: &str, cmd: Option<&str>| MarketSkill { link: link.into(), install_command: cmd.map(str::to_string), ..Default::default() };
        // a whole repo
        assert_eq!(source(&with("https://github.com/Leonxlnx/taste-skill", Some("npx skills add Leonxlnx/taste-skill"))).as_deref(), Some("Leonxlnx/taste-skill"));
        // one skill, named by the command
        assert_eq!(source(&with("https://github.com/wshobson/agents", Some("npx skills add wshobson/agents --skill react-state-management"))).as_deref(), Some("wshobson/agents#react-state-management"));
        // the link's folder wins over a command that names the SKILL.md instead of a skill
        assert_eq!(
            source(&with("https://github.com/gamedev-skills/awesome-gamedev-agent-skills/blob/main/skills/godot-gdscript/SKILL.md", Some("npx skills add gamedev-skills/awesome-gamedev-agent-skills --skill SKILL.md"))).as_deref(),
            Some("gamedev-skills/awesome-gamedev-agent-skills#skills/godot-gdscript")
        );
        assert_eq!(source(&with("https://github.com/ComposioHQ/awesome-claude-skills/tree/master/content-research-writer", None)).as_deref(), Some("ComposioHQ/awesome-claude-skills#content-research-writer"));
        assert_eq!(source(&with("https://github.com/x/y", Some("npx skills add x/y --skill SKILL.md"))).as_deref(), Some("x/y"));
        // listing pages
        assert_eq!(source(&with("https://skillselion.com/skills/leonxlnx/taste-skill/minimalist-ui", None)).as_deref(), Some("leonxlnx/taste-skill#minimalist-ui"));
        assert_eq!(source(&with("https://www.skills.sh/jezweb/claude-skills/tailwind-v4-shadcn", None)).as_deref(), Some("jezweb/claude-skills#tailwind-v4-shadcn"));
        assert_eq!(source(&with("https://officialskills.sh/firecrawl/skills/firecrawl-build-interact", None)).as_deref(), Some("firecrawl/skills#firecrawl-build-interact"));
        assert_eq!(source(&with("https://docs.firecrawl.dev/sdks/cli", None)), None);
    }

    #[test]
    #[ignore = "network"]
    fn fetches_the_live_catalogue() {
        let _sb = crate::util::test_env::sandbox();
        // SAFETY: the sandbox holds the env lock
        unsafe { std::env::set_var("BRO_SKILL_MARKET_URL", SITE) };
        let cat = load(true).unwrap();
        assert!(cat.skills.len() > 100, "{}", cat.skills.len());
        assert!(!cat.clicks.is_empty());
        let sources = cat.skills.iter().filter(|s| source(s).is_some()).count();
        assert!(sources * 10 >= cat.skills.len() * 9, "{sources} of {}", cat.skills.len());
    }
}
