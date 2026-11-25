//! Icons for the demo's projects. Their folders aren't on disk, so nothing is there to find a favicon in or to hand
//! an agent to draw; left alone they would all show their letter. Each gets a symbol here instead, drawn the way
//! bro's own drawn icons are ([`bro_core::icon_maker`]): white line art on a 24 x 24 grid, shown on a tile in the
//! project's colour. A real folder opened in the demo that has no picture gets the folder.

use bro_core::icon_maker::{self, Drawing};
use bro_media::Rgba;

/// A folder: the project the demo has no symbol for.
const FOLDER: &str = r##"<path d="M3.5 7a2 2 0 0 1 2-2h3.6l2.2 2.5h7.2a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>"##;

/// The symbol for the demo project called `name`: its shapes on the 24 x 24 grid.
fn shapes(name: &str) -> &'static str {
    match name {
        // sessions side by side
        "bro-cli-v2" => r##"<rect x="3" y="4" width="18" height="16" rx="2.5"/><line x1="11" y1="4" x2="11" y2="20"/><line x1="11" y1="12" x2="21" y2="12"/>"##,
        // a dumbbell
        "justgains" => r##"<line x1="7" y1="12" x2="17" y2="12"/><line x1="7" y1="7" x2="7" y2="17"/><line x1="17" y1="7" x2="17" y2="17"/><line x1="3.5" y1="9.5" x2="3.5" y2="14.5"/><line x1="20.5" y1="9.5" x2="20.5" y2="14.5"/>"##,
        // a prompt
        "terminal" => r##"<polyline points="5,7 10.5,12 5,17"/><line x1="13" y1="17" x2="19" y2="17"/>"##,
        // a file whose name starts with a dot
        "dotfiles" => r##"<path d="M7 3.5h6.5L18.5 8.5V19a1.5 1.5 0 0 1-1.5 1.5H7A1.5 1.5 0 0 1 5.5 19V5A1.5 1.5 0 0 1 7 3.5z"/><polyline points="13.5,3.5 13.5,8.5 18.5,8.5"/><circle cx="9" cy="15.5" r="1.1" fill="#fff"/><line x1="12.5" y1="15.5" x2="15" y2="15.5"/>"##,
        // a microphone
        "voice" => r##"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0"/><line x1="12" y1="18" x2="12" y2="21"/>"##,
        // a picture
        "justimagine" => r##"<rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="8.5" cy="9.5" r="1.6"/><polyline points="4.5,18.5 10.5,13 14,16 16.5,14 19.5,16.5"/>"##,
        // a page in a browser
        "website-studio" => r##"<rect x="3" y="4" width="18" height="16" rx="2.5"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="7" y1="13" x2="14" y2="13"/><line x1="7" y1="16.5" x2="11" y2="16.5"/>"##,
        // a paper plane
        "justposts" => r##"<polygon points="20.5,3.5 3.5,10.5 10.5,13.5 13.5,20.5"/><line x1="10.5" y1="13.5" x2="20.5" y2="3.5"/>"##,
        // a grin
        "justmemes" => r##"<circle cx="12" cy="12" r="9"/><path d="M8 14a4.5 4.5 0 0 0 8 0"/><circle cx="9" cy="9.5" r="1.1" fill="#fff"/><circle cx="15" cy="9.5" r="1.1" fill="#fff"/>"##,
        // bars
        "analytics" => r##"<line x1="5.5" y1="20" x2="5.5" y2="13"/><line x1="12" y1="20" x2="12" y2="5"/><line x1="18.5" y1="20" x2="18.5" y2="10"/>"##,
        // a pulse
        "status-page" => r##"<polyline points="3,12 7.5,12 10,5.5 14,18.5 16.5,12 21,12"/>"##,
        // an eye
        "justwatch" => r##"<path d="M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.6"/>"##,
        // a speech bubble
        "assistant" => r##"<path d="M5 4.5h14a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2h-7l-4.5 3.5v-3.5H5a2 2 0 0 1-2-2V6.5a2 2 0 0 1 2-2z"/><circle cx="8.5" cy="10.8" r="1" fill="#fff"/><circle cx="12" cy="10.8" r="1" fill="#fff"/><circle cx="15.5" cy="10.8" r="1" fill="#fff"/>"##,
        // a toolbox
        "justtools" => r##"<rect x="3" y="8" width="18" height="12" rx="2"/><path d="M9 8V6a1.5 1.5 0 0 1 1.5-1.5h3A1.5 1.5 0 0 1 15 6v2"/><line x1="3" y1="13.5" x2="21" y2="13.5"/>"##,
        // a dashboard
        "manage-portal" => r##"<rect x="3.5" y="3.5" width="7" height="9" rx="1.5"/><rect x="13.5" y="3.5" width="7" height="5" rx="1.5"/><rect x="13.5" y="11.5" width="7" height="9" rx="1.5"/><rect x="3.5" y="15.5" width="7" height="5" rx="1.5"/>"##,
        // a phone
        "mobile" => r##"<rect x="6.5" y="2.5" width="11" height="19" rx="2.5"/><line x1="10.5" y1="17.5" x2="13.5" y2="17.5"/>"##,
        // an open book
        "docs" => r##"<path d="M12 6.5c-2-1.6-5-2-8.5-1.5v13c3.5-.5 6.5 0 8.5 1.5 2-1.5 5-2 8.5-1.5V5c-3.5-.5-6.5-.1-8.5 1.5z"/><line x1="12" y1="6.5" x2="12" y2="19.5"/>"##,
        // a megaphone
        "marketing-site" => r##"<polygon points="3.5,10 3.5,14 7.5,14 16,19 16,5 7.5,10"/><path d="M19.5 9.5a3.5 3.5 0 0 1 0 5"/>"##,
        // a card
        "billing" => r##"<rect x="2.5" y="5" width="19" height="14" rx="2.5"/><line x1="2.5" y1="10" x2="21.5" y2="10"/><line x1="6.5" y1="15" x2="10" y2="15"/>"##,
        // servers
        "infra" => r##"<rect x="3" y="3.5" width="18" height="7" rx="2"/><rect x="3" y="13.5" width="18" height="7" rx="2"/><circle cx="7" cy="7" r="1" fill="#fff"/><circle cx="7" cy="17" r="1" fill="#fff"/>"##,
        // layers
        "design-kit" => r##"<polygon points="12,3.5 20.5,8 12,12.5 3.5,8"/><polyline points="3.5,12 12,16.5 20.5,12"/><polyline points="3.5,16 12,20.5 20.5,16"/>"##,
        // a flag
        "release-notes" => r##"<line x1="5" y1="21" x2="5" y2="3.5"/><path d="M5 4.5c4-2 7 2 14 0v9c-7 2-10-2-14 0"/>"##,
        _ => FOLDER,
    }
}

/// The demo project `name`'s icon as bro composes a drawn one (a white symbol on clear, `side` pixels square).
pub fn picture(name: &str, side: u32) -> Option<Rgba> {
    let svg = icon_maker::compose(&Drawing { shapes: shapes(name).to_string() });
    bro_media::svg_thumbnail(svg.as_bytes(), side).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_project_has_a_symbol_that_draws() {
        let names: Vec<&str> = crate::services::demo::sessions().iter().map(|s| s.project).chain(crate::services::demo::MORE_PROJECTS).chain(["dotfiles"]).collect();
        for name in names {
            assert_ne!(shapes(name), FOLDER, "{name} has no symbol of its own");
            let px = picture(name, 64).unwrap_or_else(|| panic!("{name}'s symbol doesn't draw"));
            assert_eq!((px.w, px.h), (64, 64));
            // white strokes on clear, covering enough to read and never the whole tile
            let inked = px.px.chunks_exact(4).filter(|c| c[3] > 128).count();
            assert!((120..2600).contains(&inked), "{name}: {inked} px inked");
            assert!(px.px.chunks_exact(4).filter(|c| c[3] > 128).all(|c| c[0] > 200 && c[1] > 200 && c[2] > 200), "{name} isn't white");
        }
        // a folder the demo doesn't know still gets a picture
        assert!(picture("scratch", 64).is_some());
    }
}
