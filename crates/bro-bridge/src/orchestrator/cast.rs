//! Session names from the JustGains marshmallow cast (J:\ads\AmazingAds), so a
//! person can say "tell Toast to run the tests". The orchestrator itself is
//! Hugh (Hugh G. Mass, the squad's coach); the squad names the sessions.

/// The orchestrator's own name.
pub const COACH: &str = "Hugh";

/// Handed out in order; a name is free when no live session title uses it.
pub const SQUAD: [&str; 7] = ["Mal", "Pinky", "Toast", "Minty", "Lilac", "Sky", "Bitty"];

/// Separates the name from the project in a session title: "Toast · justgains".
pub const SEPARATOR: &str = " · ";

/// The name a title carries: the part before the separator, trimmed.
pub fn name_of(title: &str) -> &str {
    title.split(SEPARATOR).next().unwrap_or(title).trim()
}

/// First squad name no title in `taken` uses; then "Mal 2", "Pinky 2", …
pub fn next_name<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let used: Vec<String> = taken.into_iter().map(|title| name_of(title).to_lowercase()).collect();
    (1..)
        .flat_map(|round| {
            SQUAD.iter().map(move |name| {
                if round == 1 { name.to_string() } else { format!("{name} {round}") }
            })
        })
        .find(|name| !used.contains(&name.to_lowercase()))
        .unwrap_or_else(|| SQUAD[0].to_string())
}

pub fn title(name: &str, project: Option<&str>) -> String {
    match project.map(str::trim).filter(|project| !project.is_empty()) {
        Some(project) => format!("{name}{SEPARATOR}{project}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_handed_out_in_order_and_reused_when_free() {
        assert_eq!(next_name([]), "Mal");
        assert_eq!(next_name(["Mal · justgains", "pwsh"]), "Pinky");
        assert_eq!(next_name(["pinky", "Mal"]), "Toast");
        let all: Vec<&str> = SQUAD.to_vec();
        assert_eq!(next_name(all), "Mal 2");
        assert_eq!(title("Toast", Some("justgains")), "Toast · justgains");
        assert_eq!(name_of("Toast · justgains"), "Toast");
    }
}
