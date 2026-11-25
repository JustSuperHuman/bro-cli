use std::path::Path;

/// A path as the parts an address bar shows: "~" for your home, then each folder ("F:\code\bro" → F:, code,
/// bro; "/opt/x" → /, opt, x).
pub(crate) fn path_parts(p: &Path) -> Vec<String> {
    use std::path::Component;
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Prefix(x) => Some(x.as_os_str().to_string_lossy().into_owned()),
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                Component::ParentDir => Some("..".into()),
                Component::RootDir | Component::CurDir => None,
            })
            .collect()
    };
    let all = parts(p);
    let same = |a: &String, b: &String| {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if let Some(home) = dirs::home_dir() {
        let h = parts(&home);
        if !h.is_empty() && all.len() >= h.len() && all.iter().zip(&h).all(|(a, b)| same(a, b)) {
            return std::iter::once("~".to_string())
                .chain(all[h.len()..].iter().cloned())
                .collect();
        }
    }
    // a Unix root shows as "/"
    if p.has_root() && !matches!(p.components().next(), Some(Component::Prefix(_))) {
        return std::iter::once("/".to_string()).chain(all).collect();
    }
    all
}

/// Which breadcrumbs fit `room` (each part `widths[i]` wide, the "…" `ell`, a separator between each): the parts
/// to show in order, None for the "…". All of them when they fit; else the first, "…" and as many of the last
/// ones as fit; else "…" and the last; else the last alone (cut short as it's drawn).
pub(crate) fn crumb_plan(widths: &[f32], ell: f32, gap: f32, room: f32) -> Vec<Option<usize>> {
    let n = widths.len();
    if n == 0 {
        return vec![];
    }
    let width = |plan: &[Option<usize>]| {
        plan.iter()
            .map(|p| p.map(|i| widths[i]).unwrap_or(ell))
            .sum::<f32>()
            + gap * plan.len().saturating_sub(1) as f32
    };
    let mut plans: Vec<Vec<Option<usize>>> = vec![(0..n).map(Some).collect()];
    for k in (1..n.saturating_sub(1)).rev() {
        plans.push(
            [Some(0), None]
                .into_iter()
                .chain((n - k..n).map(Some))
                .collect(),
        );
    }
    if n > 1 {
        plans.push(vec![None, Some(n - 1)]);
    }
    plans
        .into_iter()
        .find(|p| width(p) <= room)
        .unwrap_or_else(|| vec![Some(n - 1)])
}
