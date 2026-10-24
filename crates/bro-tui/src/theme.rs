//! Colour themes (ported from z4-oriel). An accent theme sets a handful of colours; background and body text
//! come from the terminal itself (`Color::Reset`) so bro looks native anywhere. A painted theme also brings
//! its own background, text and 16-colour terminal palette, so the agents' output is recoloured with it.
//!
//! * built-in accent palettes: `graphite` (the calm, neutral default), oriel's and `ultra`
//! * painted: `paper` and bro's own `nebula`, plus the most-installed VS Code themes, taken from their published
//!   colour files (editor, side bar, cursor and all 16 terminal colours): GitHub (`github-dark`, `github-dimmed`,
//!   `github-light`), `one-dark-pro`, `one-light`, `dracula`, `monokai-pro`, `night-owl`, `night-owl-light`,
//!   Ayu (`ayu-dark`, `ayu-mirage`, `ayu-light`), `palenight`, `tokyo-night`, `tokyo-night-storm`, Catppuccin
//!   (`catppuccin-mocha`, `-macchiato`, `-frappe`, `-latte`), `rose-pine`, `rose-pine-dawn`, `kanagawa`,
//!   `gruvbox`, `nord`, `everforest`, `solarized-dark`, `solarized-light`, `poimandres`, `vitesse-dark`,
//!   `noctis`, `aura`, `andromeda`, `synthwave-84`, `shades-of-purple`, `cobalt2`, `panda` and VS Code's own
//!   `dark-modern`
//! * skinned (painted, plus borders, titles, a backdrop and screen effects — see [`crate::skin`]): `phosphor`,
//!   `outrun`, `blueprint`, `cyber`, `digital-rain`, `vaporwave`
//! * `terminal`: ANSI colours only, so it follows the terminal's own theme
//! * your own: `~/.bro/themes/<name>.toml`, live-reloaded when the file changes:
//!
//! ```toml
//! base = "ultra"      # start from any theme; only list what you change
//! accent = "#ff79c6"
//! rainbow = false
//! ansi = ["#000000", "#ff5555", ...]  # optional: all 16 terminal colours (black..white, then bright)
//! panel = "#16161e"   # optional: the desktop app's side bar and window, around the sessions
//! cursor = "#f5e0dc"  # optional: the desktop app's terminal cursor
//! skin = "cyber"      # optional: borrow a built-in theme's borders, titles, backdrop and effects
//! borders = "double"  # optional: rounded, plain, double, thick, dashed, heavy-dashed, ascii, block
//! ```

use ratatui::style::Color;
use std::path::PathBuf;

/// The colours every surface draws with.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    /// Reset = the terminal's own background
    pub bg: Color,
    /// body text
    pub fg: Color,
    /// focused frames, titles, highlights
    pub accent: Color,
    /// secondary highlight
    pub shine: Color,
    /// unfocused borders
    pub frame: Color,
    /// hints, metadata
    pub muted: Color,
    /// selection background tint
    pub user: Color,
    /// warm highlight (warnings, inline values)
    pub inline: Color,
    pub danger: Color,
    pub good: Color,
    /// rainbow logo + title (ultra)
    pub animated: bool,
    /// The sessions' 16 ANSI colours (black, red, green, yellow, blue, magenta, cyan, white, then bright);
    /// None leaves them to the terminal.
    pub ansi: Option<[Color; 16]>,
    /// The chrome around the sessions when it differs from `bg` (VS Code's side bar): the desktop app's window,
    /// with the sessions on `bg`. None steps the sessions off `bg` instead.
    pub panel: Option<Color>,
    /// The desktop app's terminal cursor; None blends one from the accent.
    pub cursor: Option<Color>,
    /// Borders, titles, backdrop and screen effects.
    pub skin: crate::skin::Skin,
}

// name, accent, shine, frame, muted, user, inline, good, danger
/// (name, accent, shine, frame, muted, user, inline, good, danger)
type Palette = (&'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str, &'static str);

const PALETTES: &[Palette] = &[
    // neutral: greys with a cool off-white accent — the harness colours carry the colour
    ("graphite", "#d9dde4", "#9fb4cc", "#34363b", "#7c7f87", "#4a4d55", "#e3c27d", "#8fcf9c", "#ef7b7b"),
    ("ultra", "#b48cff", "#8be9fd", "#3d3852", "#7d7896", "#5c5480", "#ff9ad5", "#7dffc8", "#ff6b9d"),
    ("oriel", "#d4884a", "#ffd2a8", "#3c3c3c", "#6e6e6e", "#555555", "#e6b673", "#9cc46a", "#e0694a"),
    ("ember", "#ff5f3a", "#ffc7a8", "#4a2a24", "#7a5c55", "#6a3a30", "#ff9b6b", "#c2cc5a", "#ff4a3a"),
    ("ocean", "#4aa8d4", "#b8e6ff", "#24384a", "#5c6e7a", "#35556a", "#7fd0ff", "#4fd6b0", "#ff7a8a"),
    ("forest", "#6fbf73", "#c8f5c0", "#2a3d2b", "#5e7360", "#3f5a41", "#a6e3a1", "#8fe07a", "#e08a5a"),
    ("sakura", "#f28fb5", "#ffd6e7", "#4a2d3a", "#86697a", "#6a4256", "#ffb3d0", "#8fe0bc", "#ff5f8f"),
    ("synthwave", "#ff4fd8", "#7df9ff", "#3a2360", "#7a6a9a", "#5a3a90", "#7df9ff", "#5af2c0", "#ff4f7b"),
    ("matrix", "#39ff6a", "#c8ffd5", "#12361d", "#3f7a52", "#1f5a30", "#7dff9e", "#39ff6a", "#ff5a4a"),
    ("amber", "#ffb000", "#ffe0a0", "#4a3500", "#8a6a2a", "#6a4c00", "#ffcc55", "#c8d65a", "#ff6a3a"),
    ("mono", "#e0e0e0", "#ffffff", "#444444", "#777777", "#5a5a5a", "#cfcfcf", "#d8d8d8", "#8c8c8c"),
];

/// A theme that paints everything: its own background, text and terminal palette.
struct Painted {
    name: &'static str,
    bg: &'static str,
    fg: &'static str,
    accent: &'static str,
    shine: &'static str,
    frame: &'static str,
    muted: &'static str,
    user: &'static str,
    inline: &'static str,
    good: &'static str,
    danger: &'static str,
    rainbow: bool,
    /// the window around the sessions (VS Code's side bar), when it isn't `bg`
    panel: Option<&'static str>,
    /// the terminal cursor, when the theme has its own
    cursor: Option<&'static str>,
    /// black, red, green, yellow, blue, magenta, cyan, white, then the bright eight
    ansi: [&'static str; 16],
}

#[rustfmt::skip]
const PAINTED: &[Painted] = &[
    // Tokyo Night: neon signs over a rainy city at night
    Painted {
        name: "tokyo-night",
        bg: "#1a1b26", fg: "#c0caf5", accent: "#7aa2f7", shine: "#bb9af7", frame: "#3b4261", muted: "#565f89",
        user: "#283457", inline: "#e0af68", good: "#9ece6a", danger: "#f7768e", rainbow: false,
        panel: Some("#16161e"), cursor: Some("#c0caf5"),
        ansi: ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
               "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5"],
    },
    // Tokyo Night Storm: the lighter, stormier night over the city
    Painted {
        name: "tokyo-night-storm",
        bg: "#24283b", fg: "#c0caf5", accent: "#7aa2f7", shine: "#bb9af7", frame: "#3b4261", muted: "#565f89",
        user: "#2e3c64", inline: "#e0af68", good: "#9ece6a", danger: "#f7768e", rainbow: false,
        panel: Some("#1f2335"), cursor: Some("#c0caf5"),
        ansi: ["#1d202f", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
               "#414868", "#ff899d", "#9fe044", "#faba4a", "#8db0ff", "#c7a9ff", "#a4daff", "#c0caf5"],
    },
    // Catppuccin Mocha: the darkest roast of the pastel favourite
    Painted {
        name: "catppuccin-mocha",
        bg: "#1e1e2e", fg: "#cdd6f4", accent: "#cba6f7", shine: "#89dceb", frame: "#45475a", muted: "#7f849c",
        user: "#313244", inline: "#fab387", good: "#a6e3a1", danger: "#f38ba8", rainbow: false,
        panel: Some("#181825"), cursor: Some("#f5e0dc"),
        ansi: ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
               "#585b70", "#f37799", "#89d88b", "#ebd391", "#74a8fc", "#f2aede", "#6bd7ca", "#a6adc8"],
    },
    // Catppuccin Macchiato: the medium roast, soothing pastels
    Painted {
        name: "catppuccin-macchiato",
        bg: "#24273a", fg: "#cad3f5", accent: "#c6a0f6", shine: "#91d7e3", frame: "#494d64", muted: "#8087a2",
        user: "#363a4f", inline: "#f5a97f", good: "#a6da95", danger: "#ed8796", rainbow: false,
        panel: Some("#1e2030"), cursor: Some("#f4dbd6"),
        ansi: ["#494d64", "#ed8796", "#a6da95", "#eed49f", "#8aadf4", "#f5bde6", "#8bd5ca", "#a5adcb",
               "#5b6078", "#ec7486", "#8ccf7f", "#e1c682", "#78a1f6", "#f2a9dd", "#63cbc0", "#b8c0e0"],
    },
    // Catppuccin Frappé: the gentlest of the dark flavours
    Painted {
        name: "catppuccin-frappe",
        bg: "#303446", fg: "#c6d0f5", accent: "#ca9ee6", shine: "#99d1db", frame: "#51576d", muted: "#838ba7",
        user: "#414559", inline: "#ef9f76", good: "#a6d189", danger: "#e78284", rainbow: false,
        panel: Some("#292c3c"), cursor: Some("#f2d5cf"),
        ansi: ["#51576d", "#e78284", "#a6d189", "#e5c890", "#8caaee", "#f4b8e4", "#81c8be", "#a5adce",
               "#626880", "#e67172", "#8ec772", "#d9ba73", "#7b9ef0", "#f2a4db", "#5abfb5", "#b5bfe2"],
    },
    // GitHub Dark Default: near-black blue with GitHub's blue and violet
    Painted {
        name: "github-dark",
        bg: "#0d1117", fg: "#e6edf3", accent: "#2f81f7", shine: "#d2a8ff", frame: "#30363d", muted: "#7d8590",
        user: "#173355", inline: "#ffa657", good: "#3fb950", danger: "#f85149", rainbow: false,
        panel: Some("#010409"), cursor: Some("#2f81f7"),
        ansi: ["#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
               "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#ffffff"],
    },
    // GitHub Dark Dimmed: soft slate, easy on the eyes all day
    Painted {
        name: "github-dimmed",
        bg: "#22272e", fg: "#adbac7", accent: "#539bf5", shine: "#dcbdfb", frame: "#444c56", muted: "#768390",
        user: "#2e445f", inline: "#f69d50", good: "#57ab5a", danger: "#e5534b", rainbow: false,
        panel: Some("#1c2128"), cursor: Some("#539bf5"),
        ansi: ["#545d68", "#f47067", "#57ab5a", "#c69026", "#539bf5", "#b083f0", "#39c5cf", "#909dab",
               "#636e7b", "#ff938a", "#6bc46d", "#daaa3f", "#6cb6ff", "#dcbdfb", "#56d4dd", "#cdd9e5"],
    },
    // One Dark Pro: Atom's classic, the most installed dark theme there is
    Painted {
        name: "one-dark-pro",
        bg: "#282c34", fg: "#abb2bf", accent: "#61afef", shine: "#c678dd", frame: "#3e4452", muted: "#7f848e",
        user: "#3e4451", inline: "#e5c07b", good: "#98c379", danger: "#e06c75", rainbow: false,
        panel: Some("#21252b"), cursor: Some("#528bff"),
        ansi: ["#3f4451", "#e05561", "#8cc265", "#d18f52", "#4aa5f0", "#c162de", "#42b3c2", "#d7dae0",
               "#4f5666", "#ff616e", "#a5e075", "#f0a45d", "#4dc4ff", "#de73ff", "#4cd1e0", "#e6e6e6"],
    },
    // Dracula: the official palette, purple and pink after dark
    Painted {
        name: "dracula",
        bg: "#282a36", fg: "#f8f8f2", accent: "#bd93f9", shine: "#ff79c6", frame: "#44475a", muted: "#6272a4",
        user: "#44475a", inline: "#ffb86c", good: "#50fa7b", danger: "#ff5555", rainbow: false,
        panel: Some("#21222c"), cursor: Some("#f8f8f2"),
        ansi: ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
               "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
    },
    // Monokai Pro: warm charcoal and the famous pastel six
    Painted {
        name: "monokai-pro",
        bg: "#2d2a2e", fg: "#fcfcfa", accent: "#ffd866", shine: "#78dce8", frame: "#5b595c", muted: "#939293",
        user: "#403e41", inline: "#fc9867", good: "#a9dc76", danger: "#ff6188", rainbow: false,
        panel: Some("#221f22"), cursor: Some("#fcfcfa"),
        ansi: ["#403e41", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa",
               "#727072", "#ff6188", "#a9dc76", "#ffd866", "#fc9867", "#ab9df2", "#78dce8", "#fcfcfa"],
    },
    // Night Owl: deep navy tuned for coding late into the night
    Painted {
        name: "night-owl",
        bg: "#011627", fg: "#d6deeb", accent: "#82aaff", shine: "#c792ea", frame: "#1d3b53", muted: "#637777",
        user: "#1d3b53", inline: "#ecc48d", good: "#22da6e", danger: "#ef5350", rainbow: false,
        panel: None, cursor: Some("#80a4c2"),
        ansi: ["#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff",
               "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff"],
    },
    // Dark Modern: VS Code's own default, right at home
    Painted {
        name: "dark-modern",
        bg: "#1f1f1f", fg: "#cccccc", accent: "#4daafc", shine: "#4ec9b0", frame: "#3c3c3c", muted: "#9d9d9d",
        user: "#264f78", inline: "#ce9178", good: "#81b88b", danger: "#f85149", rainbow: false,
        panel: Some("#181818"), cursor: Some("#aeafad"),
        ansi: ["#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
               "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#e5e5e5"],
    },
    // Ayu Dark: ink black with marigold, crisp and calm
    Painted {
        name: "ayu-dark",
        bg: "#10141c", fg: "#bfbdb6", accent: "#e6b450", shine: "#59c2ff", frame: "#2d3441", muted: "#5a6378",
        user: "#193155", inline: "#ff8f40", good: "#aad94c", danger: "#d95757", rainbow: false,
        panel: Some("#0d1017"), cursor: Some("#e6b450"),
        ansi: ["#1b1f29", "#f06b73", "#70bf56", "#fdb04c", "#4fbfff", "#d0a1ff", "#93e2c8", "#c7c7c7",
               "#686868", "#f07178", "#aad94c", "#ffb454", "#59c2ff", "#d2a6ff", "#95e6cb", "#ffffff"],
    },
    // Ayu Mirage: dusky blue-grey with a golden glow
    Painted {
        name: "ayu-mirage",
        bg: "#242936", fg: "#cccac2", accent: "#ffcc66", shine: "#73d0ff", frame: "#424a5b", muted: "#707a8c",
        user: "#2b4668", inline: "#ffa659", good: "#87d96c", danger: "#ff6666", rainbow: false,
        panel: Some("#1f2430"), cursor: Some("#ffcc66"),
        ansi: ["#171b24", "#f28273", "#87d96c", "#fcca60", "#6acdff", "#ddbbff", "#93e2c8", "#c7c7c7",
               "#686868", "#f28779", "#d5ff80", "#ffcd66", "#73d0ff", "#dfbfff", "#95e6cb", "#ffffff"],
    },
    // Palenight: Material's soft violet night
    Painted {
        name: "palenight",
        bg: "#292d3e", fg: "#bfc7d5", accent: "#c792ea", shine: "#89ddff", frame: "#4c5374", muted: "#697098",
        user: "#414764", inline: "#f78c6c", good: "#c3e88d", danger: "#ff5572", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#676e95", "#ff5572", "#a9c77d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
               "#676e95", "#ff5572", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff"],
    },
    // Rosé Pine: soho vibes, muted pine, rose and gold
    Painted {
        name: "rose-pine",
        bg: "#191724", fg: "#e0def4", accent: "#c4a7e7", shine: "#ebbcba", frame: "#403d52", muted: "#6e6a86",
        user: "#2a283e", inline: "#f6c177", good: "#9ccfd8", danger: "#eb6f92", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#26233a", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4",
               "#6e6a86", "#eb6f92", "#31748f", "#f6c177", "#9ccfd8", "#c4a7e7", "#ebbcba", "#e0def4"],
    },
    // Kanagawa: Hokusai's Great Wave in ink and indigo
    Painted {
        name: "kanagawa",
        bg: "#1f1f28", fg: "#dcd7ba", accent: "#7e9cd8", shine: "#7fb4ca", frame: "#54546d", muted: "#727169",
        user: "#2d4f67", inline: "#e6c384", good: "#98bb6c", danger: "#ff5d62", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#16161d", "#c34043", "#76946a", "#c0a36e", "#7e9cd8", "#957fb8", "#6a9589", "#c8c093",
               "#727169", "#e82424", "#98bb6c", "#e6c384", "#7fb4ca", "#938aa9", "#7aa89f", "#dcd7ba"],
    },
    // Gruvbox: retro groove, warm and earthy
    Painted {
        name: "gruvbox",
        bg: "#282828", fg: "#ebdbb2", accent: "#fe8019", shine: "#8ec07c", frame: "#504945", muted: "#928374",
        user: "#3c3836", inline: "#fabd2f", good: "#b8bb26", danger: "#fb4934", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
               "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2"],
    },
    // Nord: an arctic, north-bluish calm
    Painted {
        name: "nord",
        bg: "#2e3440", fg: "#d8dee9", accent: "#88c0d0", shine: "#81a1c1", frame: "#4c566a", muted: "#7b88a1",
        user: "#434c5e", inline: "#ebcb8b", good: "#a3be8c", danger: "#bf616a", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
               "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
    },
    // Everforest: soft greens of a forest at dusk
    Painted {
        name: "everforest",
        bg: "#2d353b", fg: "#d3c6aa", accent: "#a7c080", shine: "#83c092", frame: "#4f585e", muted: "#859289",
        user: "#3d484d", inline: "#dbbc7f", good: "#a7c080", danger: "#e67e80", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#343f44", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa",
               "#859289", "#e67e80", "#a7c080", "#dbbc7f", "#7fbbb3", "#d699b6", "#83c092", "#d3c6aa"],
    },
    // Solarized Dark: Ethan Schoonover's precision palette
    Painted {
        name: "solarized-dark",
        bg: "#002b36", fg: "#839496", accent: "#268bd2", shine: "#2aa198", frame: "#1d4a55", muted: "#586e75",
        user: "#274642", inline: "#b58900", good: "#859900", danger: "#dc322f", rainbow: false,
        panel: Some("#00212b"), cursor: Some("#93a1a1"),
        ansi: ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
               "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
    },
    // Poimandres: muted midnight blue with sea-glass mint
    Painted {
        name: "poimandres",
        bg: "#1b1e28", fg: "#a6accd", accent: "#5de4c7", shine: "#add7ff", frame: "#303340", muted: "#767c9d",
        user: "#303340", inline: "#fffac2", good: "#5de4c7", danger: "#d0679d", rainbow: false,
        panel: None, cursor: Some("#a6accd"),
        ansi: ["#1b1e28", "#d0679d", "#5de4c7", "#fffac2", "#89ddff", "#f087bd", "#89ddff", "#ffffff",
               "#a6accd", "#d0679d", "#5de4c7", "#fffac2", "#add7ff", "#f087bd", "#add7ff", "#ffffff"],
    },
    // Vitesse Dark: Anthony Fu's quiet, earthy minimalism
    Painted {
        name: "vitesse-dark",
        bg: "#121212", fg: "#dbd7ca", accent: "#4d9375", shine: "#c98a7d", frame: "#2e2e2e", muted: "#758575",
        user: "#272727", inline: "#e6cc77", good: "#80a665", danger: "#cb7676", rainbow: false,
        panel: None, cursor: Some("#dbd7ca"),
        ansi: ["#393a34", "#cb7676", "#4d9375", "#e6cc77", "#6394bf", "#d9739f", "#5eaab5", "#dbd7ca",
               "#777777", "#cb7676", "#4d9375", "#e6cc77", "#6394bf", "#d9739f", "#5eaab5", "#ffffff"],
    },
    // Noctis: deep sea teal with coral and mint
    Painted {
        name: "noctis",
        bg: "#052529", fg: "#b2cacd", accent: "#40d4e7", shine: "#df769b", frame: "#0a4c54", muted: "#5b858b",
        user: "#0a464d", inline: "#e4b781", good: "#49e9a6", danger: "#e66533", rainbow: false,
        panel: Some("#041d20"), cursor: Some("#85efff"),
        ansi: ["#324a4d", "#e66533", "#49e9a6", "#e4b781", "#49ace9", "#df769b", "#49d6e9", "#b2cacd",
               "#47686c", "#e97749", "#60ebb1", "#e69533", "#60b6eb", "#e798b3", "#60dbeb", "#c1d4d7"],
    },
    // Aura: deep violet dark with mint and peach
    Painted {
        name: "aura",
        bg: "#15141b", fg: "#edecee", accent: "#a277ff", shine: "#61ffca", frame: "#2e2b38", muted: "#6d6d6d",
        user: "#29263c", inline: "#ffca85", good: "#61ffca", danger: "#ff6767", rainbow: false,
        panel: Some("#110f18"), cursor: Some("#a277ff"),
        ansi: ["#29263c", "#ff6767", "#61ffca", "#ffca85", "#a277ff", "#f694ff", "#82e2ff", "#cdccce",
               "#6d6d6d", "#ff6767", "#61ffca", "#ffca85", "#a277ff", "#f694ff", "#82e2ff", "#edecee"],
    },
    // Andromeda: a dark galaxy of electric teal, violet and gold
    Painted {
        name: "andromeda",
        bg: "#23262e", fg: "#d5ced9", accent: "#00e8c6", shine: "#c74ded", frame: "#3d4352", muted: "#746f77",
        user: "#3d4352", inline: "#f39c12", good: "#96e072", danger: "#ee5d43", rainbow: false,
        panel: None, cursor: Some("#ffe66d"),
        ansi: ["#1b1d23", "#ee5d43", "#96e072", "#ffe66d", "#7cb7ff", "#ff00aa", "#00e8c6", "#d5ced9",
               "#746f77", "#ee5d43", "#96e072", "#ffe66d", "#7cb7ff", "#ff00aa", "#00e8c6", "#ffffff"],
    },
    // SynthWave '84: neon that glows, straight out of 1984
    Painted {
        name: "synthwave-84",
        bg: "#262335", fg: "#f6f4fb", accent: "#ff7edb", shine: "#36f9f6", frame: "#495495", muted: "#848bbd",
        user: "#413e4e", inline: "#fede5d", good: "#72f1b8", danger: "#fe4450", rainbow: true,
        panel: Some("#241b2f"), cursor: Some("#f97e72"),
        ansi: ["#2a2139", "#fe4450", "#72f1b8", "#f3e70f", "#2ee2fa", "#ff7edb", "#03edf9", "#f6f4fb",
               "#848bbd", "#fe4450", "#72f1b8", "#fede5d", "#36f9f6", "#ff7edb", "#03edf9", "#ffffff"],
    },
    // Shades of Purple: bold purple with a hit of yellow
    Painted {
        name: "shades-of-purple",
        bg: "#2d2b55", fg: "#ffffff", accent: "#fad000", shine: "#9effff", frame: "#514c81", muted: "#a599e9",
        user: "#5d3fa0", inline: "#ff9d00", good: "#a5ff90", danger: "#ec3a37", rainbow: false,
        panel: Some("#222244"), cursor: Some("#fad000"),
        ansi: ["#000000", "#ec3a37", "#3ad900", "#fad000", "#7857fe", "#ff2c70", "#80fcff", "#ffffff",
               "#5c5c61", "#ec3a37", "#3ad900", "#fad000", "#6943ff", "#fb94ff", "#80fcff", "#ffffff"],
    },
    // Cobalt2: Wes Bos's electric blue and warning-sign yellow
    Painted {
        name: "cobalt2",
        bg: "#193549", fg: "#e1efff", accent: "#ffc600", shine: "#0088ff", frame: "#1f4662", muted: "#aaaaaa",
        user: "#0050a4", inline: "#ff9d00", good: "#3ad900", danger: "#ff628c", rainbow: false,
        panel: Some("#15232d"), cursor: Some("#ffc600"),
        ansi: ["#000000", "#ff628c", "#3ad900", "#ffc600", "#0088ff", "#fb94ff", "#80fcff", "#ffffff",
               "#0050a4", "#ff628c", "#3ad900", "#ffc600", "#0088ff", "#fb94ff", "#80fcff", "#ffffff"],
    },
    // Panda: charcoal with candy pink and mint
    Painted {
        name: "panda",
        bg: "#292a2b", fg: "#e6e6e6", accent: "#19f9d8", shine: "#ff75b5", frame: "#3b3e43", muted: "#676b79",
        user: "#5e4e3b", inline: "#ffb86c", good: "#19f9d8", danger: "#ff2c6d", rainbow: false,
        panel: Some("#242526"), cursor: Some("#ff4b82"),
        ansi: ["#2a2b2d", "#ff2c6d", "#19f9d8", "#ffb86c", "#45a9f9", "#ff75b5", "#b084eb", "#cdcdcd",
               "#757575", "#ff2c6d", "#19f9d8", "#ffcc95", "#6fc1ff", "#ff9ac1", "#bcaafe", "#e6e6e6"],
    },
    // bro's own: deep space, neon edges, the rainbow logo
    Painted {
        name: "nebula",
        bg: "#0e0b1e", fg: "#e6defe", accent: "#a682ff", shine: "#5ce1ff", frame: "#2f2756", muted: "#7a70a8",
        user: "#261f4a", inline: "#ffb86b", good: "#6dffb3", danger: "#ff5c8a", rainbow: true,
        panel: None, cursor: None,
        ansi: ["#1b1636", "#ff5c8a", "#6dffb3", "#ffd76b", "#6b9bff", "#c77dff", "#5ce1ff", "#d8d0f5",
               "#4a4078", "#ff85a8", "#9dffcc", "#ffe599", "#97b8ff", "#dca8ff", "#8debff", "#ffffff"],
    },
    // CRT terminal: everything in green phosphor, scanlines, a dark vignette
    Painted {
        name: "phosphor",
        bg: "#020a04", fg: "#3dff7d", accent: "#8affab", shine: "#c4ffd6", frame: "#1d6b35", muted: "#2f9a52",
        user: "#0c3018", inline: "#b0ffc6", good: "#3dff7d", danger: "#d8ffe4", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#03140a", "#a8ffbe", "#3dff7d", "#c4ffd6", "#2fc864", "#7dffa6", "#5cff92", "#3dff7d",
               "#1d6b35", "#c4ffd6", "#6dff9a", "#e0ffe9", "#4ae07a", "#9dffbb", "#7dffa6", "#e8fff0"],
    },
    // synthwave sunset: a purple gradient sky full of stars, neon edges
    Painted {
        name: "outrun",
        bg: "#12002b", fg: "#f5e6ff", accent: "#ff2e97", shine: "#00e5ff", frame: "#5a2380", muted: "#a07cc0",
        user: "#2e0a4a", inline: "#ffd319", good: "#3cf0a0", danger: "#ff3864", rainbow: true,
        panel: None, cursor: None,
        ansi: ["#2b0a3d", "#fe4450", "#72f1b8", "#fede5d", "#03edf9", "#ff7edb", "#36f9f6", "#f5e6ff",
               "#614d85", "#ff6b78", "#9bf7d0", "#ffeb8f", "#5cf3fb", "#ffa6e8", "#7afbf8", "#ffffff"],
    },
    // engineering drawing: white ink on blueprint blue, graph paper, dashed construction lines
    Painted {
        name: "blueprint",
        bg: "#0b2e5c", fg: "#e8f1ff", accent: "#ffffff", shine: "#8fd3ff", frame: "#5b8cc8", muted: "#9ab9e0",
        user: "#174a85", inline: "#ffe08a", good: "#9ef0c0", danger: "#ff9a8a", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#0a2548", "#ff9a8a", "#9ef0c0", "#ffe08a", "#8fd3ff", "#d7b8ff", "#7fe8e8", "#e8f1ff",
               "#5d86bb", "#ffb8ab", "#bff7d6", "#fff0b8", "#b9e4ff", "#e6d1ff", "#a8f2f2", "#ffffff"],
    },
    // night city HUD: black and hazard yellow, solid title tabs, heavy frames
    Painted {
        name: "cyber",
        bg: "#07070c", fg: "#e6f7ff", accent: "#fcee0a", shine: "#00f0ff", frame: "#4a4a24", muted: "#8494a3",
        user: "#25250c", inline: "#ff9e00", good: "#00ff9c", danger: "#ff003c", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#0d0d14", "#ff003c", "#00ff9c", "#fcee0a", "#00b8ff", "#ff00e5", "#00f0ff", "#d0e0ea",
               "#3a3f4b", "#ff4d73", "#5cffc0", "#fff35c", "#5cd0ff", "#ff5cf0", "#7af7ff", "#ffffff"],
    },
    // there is no spoon: katakana rain falls through every empty corner
    Painted {
        name: "digital-rain",
        bg: "#000400", fg: "#b8ffc8", accent: "#00ff41", shine: "#a6ffbf", frame: "#11592a", muted: "#45995c",
        user: "#072a10", inline: "#d7ff6b", good: "#00ff41", danger: "#ff4545", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#001a06", "#ff4545", "#00ff41", "#d7ff6b", "#3cb8ff", "#c084ff", "#4dffd2", "#b8ffc8",
               "#1e5e2e", "#ff7a7a", "#66ff88", "#e8ffa0", "#7ad0ff", "#d6b0ff", "#8affe3", "#eaffef"],
    },
    // ａｅｓｔｈｅｔｉｃ: pastel neon on a dusk gradient, double frames, full-width titles
    Painted {
        name: "vaporwave",
        bg: "#1c1433", fg: "#fdf2ff", accent: "#ff71ce", shine: "#01cdfe", frame: "#6a4c9c", muted: "#ad94d6",
        user: "#3a2560", inline: "#fffb96", good: "#05ffa1", danger: "#ff4d8d", rainbow: true,
        panel: None, cursor: None,
        ansi: ["#2a1c4a", "#ff4d8d", "#05ffa1", "#fffb96", "#01cdfe", "#ff71ce", "#7af9ff", "#fdf2ff",
               "#6b55a0", "#ff7fab", "#6dffc6", "#fffdc4", "#6be3ff", "#ffa3e0", "#b2fcff", "#ffffff"],
    },
    // GitHub Light Default: the familiar white of github.com
    Painted {
        name: "github-light",
        bg: "#ffffff", fg: "#1f2328", accent: "#0969da", shine: "#8250df", frame: "#d0d7de", muted: "#656d76",
        user: "#dae9f9", inline: "#9a6700", good: "#1a7f37", danger: "#cf222e", rainbow: false,
        panel: Some("#f6f8fa"), cursor: Some("#0969da"),
        ansi: ["#24292f", "#cf222e", "#116329", "#4d2d00", "#0969da", "#8250df", "#1b7c83", "#6e7781",
               "#57606a", "#a40e26", "#1a7f37", "#633c01", "#218bff", "#a475f9", "#3192aa", "#8c959f"],
    },
    // Atom One Light: One Dark's bright twin
    Painted {
        name: "one-light",
        bg: "#fafafa", fg: "#383a42", accent: "#4078f2", shine: "#a626a4", frame: "#dbdbdc", muted: "#a0a1a7",
        user: "#e5e5e6", inline: "#c18401", good: "#50a14f", danger: "#e45649", rainbow: false,
        panel: Some("#eaeaeb"), cursor: Some("#526fff"),
        ansi: ["#383a42", "#e45649", "#50a14f", "#c18401", "#4078f2", "#a626a4", "#0184bc", "#a0a1a7",
               "#696c77", "#ca1243", "#50a14f", "#986801", "#4078f2", "#a626a4", "#0997b3", "#fafafa"],
    },
    // Night Owl Light: Night Owl for bright rooms
    Painted {
        name: "night-owl-light",
        bg: "#fbfbfb", fg: "#403f53", accent: "#2aa298", shine: "#994cc3", frame: "#d9d9d9", muted: "#90a7b2",
        user: "#e0e0e0", inline: "#c96765", good: "#08916a", danger: "#de3d3b", rainbow: false,
        panel: Some("#f0f0f0"), cursor: Some("#90a7b2"),
        ansi: ["#403f53", "#de3d3b", "#08916a", "#e0af02", "#288ed7", "#d6438a", "#2aa298", "#93a1a1",
               "#403f53", "#de3d3b", "#08916a", "#daaa01", "#288ed7", "#d6438a", "#2aa298", "#93a1a1"],
    },
    // Ayu Light: paper white with marigold and sky
    Painted {
        name: "ayu-light",
        bg: "#fcfcfc", fg: "#5c6166", accent: "#d97a00", shine: "#2f8fd8", frame: "#dadee3", muted: "#828e9f",
        user: "#d7e4f6", inline: "#e5711f", good: "#6a9a00", danger: "#e65050", rainbow: false,
        panel: Some("#f8f9fa"), cursor: Some("#f29718"),
        ansi: ["#000000", "#f06b6c", "#6cbf43", "#e7a100", "#21a1e2", "#a176cb", "#4abc96", "#c7c7c7",
               "#686868", "#f07171", "#86b300", "#eba400", "#22a4e6", "#a37acc", "#4cbf99", "#d1d1d1"],
    },
    // Solarized Light: warm parchment, the same sixteen colours
    Painted {
        name: "solarized-light",
        bg: "#fdf6e3", fg: "#657b83", accent: "#268bd2", shine: "#6c71c4", frame: "#ddd6c1", muted: "#93a1a1",
        user: "#eee8d5", inline: "#b58900", good: "#859900", danger: "#dc322f", rainbow: false,
        panel: Some("#eee8d5"), cursor: Some("#657b83"),
        ansi: ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
               "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
    },
    // Catppuccin Latte: the light roast
    Painted {
        name: "catppuccin-latte",
        bg: "#eff1f5", fg: "#4c4f69", accent: "#8839ef", shine: "#1e66f5", frame: "#bcc0cc", muted: "#7c7f93",
        user: "#ccd0da", inline: "#fe640b", good: "#40a02b", danger: "#d20f39", rainbow: false,
        panel: Some("#e6e9ef"), cursor: Some("#dc8a78"),
        ansi: ["#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
               "#6c6f85", "#de293e", "#49af3d", "#eea02d", "#456eff", "#fe85d8", "#2d9fa8", "#bcc0cc"],
    },
    // Rosé Pine Dawn: morning light on the same garden
    Painted {
        name: "rose-pine-dawn",
        bg: "#faf4ed", fg: "#575279", accent: "#907aa9", shine: "#d7827e", frame: "#dfdad9", muted: "#797593",
        user: "#f2e9e1", inline: "#ea9d34", good: "#56949f", danger: "#b4637a", rainbow: false,
        panel: None, cursor: None,
        ansi: ["#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
               "#9893a5", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279"],
    },
];

/// The default theme name.
pub const DEFAULT: &str = "graphite";

/// Every theme you can pick: built-ins, `terminal`, then your own files.
pub fn names() -> Vec<String> {
    let mut v: Vec<String> = builtin_names();
    for c in custom_names() {
        if !v.contains(&c) {
            v.push(c);
        }
    }
    v
}

/// The themes that come with bro.
pub fn builtin_names() -> Vec<String> {
    let mut v: Vec<String> = vec![PALETTES[0].0.to_string(), "paper".into()];
    v.extend(PAINTED.iter().map(|p| p.name.to_string()));
    v.extend(PALETTES[1..].iter().map(|p| p.0.to_string()));
    v.push("terminal".into());
    v
}

/// `#rrggbb` → Color.
pub fn hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#').trim_start_matches("0x");
    let s = s.get(..6)?;
    let n = u32::from_str_radix(s, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Linear blend of two RGB colours (`t` = 0 → a, 1 → b). Non-RGB colours return `a`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => a,
    }
}

/// A project's color, from its folder name alone (case-insensitive): the same project gets the same color in
/// every session, workspace and bro window, whatever else is open. Painted themes pick from their palette;
/// others from the whole hue circle, so different projects rarely share one. Never red, which reads as an error.
pub fn project_color(key: &str, t: &Theme) -> Color {
    if key == crate::panes::term::ORCHESTRATOR_KEY {
        return gold(t);
    }
    if t.name == "mono" || t.name == "terminal" {
        return t.accent;
    }
    let name = key.rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(key).to_lowercase();
    // FNV-1a, explicitly fixed instead of a platform-dependent/randomized hasher.
    let hash = name.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    // blue, magenta, cyan, yellow, green, bright blue, bright magenta, bright cyan
    if let Some(a) = t.ansi {
        return a[[4, 5, 6, 3, 2, 12, 13, 14][(hash % 8) as usize]];
    }
    let hue = 20.0 + (hash % 3200) as f32 / 10.0;
    let (saturation, lightness) = if t.is_light() { (0.55, 0.38) } else { (0.7, 0.72) };
    hsl(hue, saturation, lightness)
}

/// The orchestrator's color: gold, so its window and sidebar row stand out from every project.
pub fn gold(t: &Theme) -> Color {
    match t.name.as_str() {
        "mono" => t.accent,
        "terminal" => Color::LightYellow,
        _ if t.is_light() => Color::Rgb(178, 128, 0),
        _ => Color::Rgb(255, 200, 61),
    }
}

fn hsl(hue: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = hue / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let v = |n: f32| ((n + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(v(r), v(g), v(b))
}

pub fn project_tint(color: Color, t: &Theme) -> Color {
    mix(color, if t.is_light() { t.bg } else { t.deep() }, if t.is_light() { 0.9 } else { 0.83 })
}

/// Look a theme up by name (unknown names fall back to the default palette).
pub fn get(name: &str) -> Theme {
    get_depth(name, 0)
}

fn get_depth(name: &str, depth: usize) -> Theme {
    if !builtin_names().iter().any(|b| b == name)
        && let Some(t) = custom(name, depth) {
            return t;
        }
    builtin(name)
}

fn builtin(name: &str) -> Theme {
    if name == "terminal" {
        return terminal();
    }
    if name == "paper" {
        return paper();
    }
    if let Some(p) = PAINTED.iter().find(|p| p.name == name) {
        return painted(p);
    }
    let p = PALETTES.iter().find(|p| p.0 == name).unwrap_or(&PALETTES[0]);
    let c = |s| hex(s).unwrap_or(Color::Reset);
    // borders and hints lean grey so the theme colour is saved for what matters
    let calm = |s, grey: Color, a: f32| mix(c(s), grey, a);
    Theme {
        name: p.0.into(),
        bg: Color::Reset,
        fg: Color::Reset,
        accent: c(p.1),
        shine: c(p.2),
        frame: calm(p.3, Color::Rgb(58, 58, 62), 0.55),
        muted: calm(p.4, Color::Rgb(128, 128, 134), 0.5),
        user: calm(p.5, Color::Rgb(92, 92, 98), 0.45),
        inline: c(p.6),
        good: c(p.7),
        danger: c(p.8),
        animated: p.0 == "ultra",
        ansi: None,
        panel: None,
        cursor: None,
        skin: Default::default(),
    }
}

fn painted(p: &Painted) -> Theme {
    let c = |s| hex(s).unwrap_or(Color::Reset);
    Theme {
        name: p.name.into(),
        bg: c(p.bg),
        fg: c(p.fg),
        accent: c(p.accent),
        shine: c(p.shine),
        frame: c(p.frame),
        muted: c(p.muted),
        user: c(p.user),
        inline: c(p.inline),
        good: c(p.good),
        danger: c(p.danger),
        animated: p.rainbow,
        ansi: Some(p.ansi.map(c)),
        panel: p.panel.map(c),
        cursor: p.cursor.map(c),
        skin: crate::skin::for_theme(p.name),
    }
}

/// Light: bro paints its own white background (and the sessions' default colours with it).
fn paper() -> Theme {
    let c = |s| hex(s).unwrap_or(Color::Reset);
    Theme {
        name: "paper".into(),
        bg: c("#fbfbfa"),
        fg: c("#24292f"),
        accent: c("#1f2328"),
        shine: c("#0969da"),
        frame: c("#d0d7de"),
        muted: c("#6e7781"),
        user: c("#dbe4ee"),
        inline: c("#9a6700"),
        good: c("#1a7f37"),
        danger: c("#cf222e"),
        animated: false,
        ansi: None,
        panel: None,
        cursor: None,
        skin: Default::default(),
    }
}

impl Theme {
    /// A light theme (dark text on a painted light background).
    pub fn is_light(&self) -> bool {
        match self.bg {
            Color::Rgb(r, g, b) => (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 > 140,
            _ => false,
        }
    }

    /// Paints its own background.
    pub fn painted(&self) -> bool {
        !matches!(self.bg, Color::Reset)
    }

    /// The darkest surface: a painted dark theme's background, else a near-black that suits any dark terminal.
    pub fn deep(&self) -> Color {
        if self.painted() && !self.is_light() { self.bg } else { Color::Rgb(18, 18, 22) }
    }

    /// A few colours that sum the theme up, for swatches.
    pub fn swatch(&self) -> [Color; 5] {
        [self.accent, self.shine, self.good, self.inline, self.danger]
    }
}

/// ANSI-only: every colour is one of the terminal's 16.
fn terminal() -> Theme {
    Theme {
        name: "terminal".into(),
        bg: Color::Reset,
        fg: Color::Reset,
        accent: Color::Blue,
        shine: Color::LightCyan,
        frame: Color::DarkGray,
        muted: Color::DarkGray,
        user: Color::DarkGray,
        inline: Color::Yellow,
        danger: Color::Red,
        good: Color::Green,
        animated: false,
        ansi: None,
        panel: None,
        cursor: None,
        skin: Default::default(),
    }
}

/// bro's theme as Claude Code takes it: bro writes it as Claude's own theme (see [`bro_core::claude_theme`]).
pub fn claude_palette(t: &Theme) -> bro_core::claude_theme::Palette {
    bro_core::claude_theme::Palette {
        id: t.name.clone(),
        base: builtin_base(&t.name),
        light: t.is_light(),
        rainbow: t.animated,
        marker: t.skin.marker.to_string(),
        bg: ink(t.bg),
        fg: ink(t.fg),
        accent: ink(t.accent),
        shine: ink(t.shine),
        frame: ink(t.frame),
        muted: ink(t.muted),
        user: ink(t.user),
        inline: ink(t.inline),
        good: ink(t.good),
        danger: ink(t.danger),
        ansi: t.ansi.map(|a| a.map(ink)),
        panel: t.panel.map(ink),
    }
}

/// A terminal colour as bro-core holds it.
fn ink(c: Color) -> bro_core::claude_theme::Ink {
    use bro_core::claude_theme::Ink;
    let ansi = |i: u8| Ink::Ansi(i);
    match c {
        Color::Rgb(r, g, b) => Ink::Rgb(r, g, b),
        Color::Reset => Ink::Terminal,
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
        Color::Indexed(i) if i < 16 => ansi(i),
        // xterm's 6×6×6 cube, then its 24 greys
        Color::Indexed(i) if i < 232 => {
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            let n = i - 16;
            Ink::Rgb(level(n / 36), level(n / 6 % 6), level(n % 6))
        }
        Color::Indexed(i) => {
            let v = 8 + (i - 232) * 10;
            Ink::Rgb(v, v, v)
        }
    }
}

/// The built-in theme `name` derives from: itself for a built-in, else the end of its file's `base` chain.
pub fn builtin_base(name: &str) -> String {
    let builtins = builtin_names();
    let mut at = name.to_string();
    for _ in 0..5 {
        if builtins.contains(&at) {
            return at;
        }
        let text = std::fs::read_to_string(file_of(&at)).unwrap_or_default();
        let base = text.parse::<toml::Table>().ok().and_then(|t| t.get("base").and_then(|v| v.as_str()).map(str::to_string));
        match base {
            Some(b) if b != at => at = b,
            _ => break,
        }
    }
    DEFAULT.to_string()
}

/// A point on the rainbow (0..1 wraps) at saturation `s`.
pub fn rainbow_at(pos: f64, s: f64) -> Color {
    let h = pos.rem_euclid(1.0) * 6.0;
    let c = s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 1.0 - c;
    let f = |u: f64| ((u + m) * 255.0) as u8;
    Color::Rgb(f(r), f(g), f(b))
}

// ------------------------------------------------------------------ your own themes

#[cfg(test)]
thread_local! {
    /// Tests keep their theme files in a folder of their own.
    pub static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Where your themes live: `~/.bro/themes`.
pub fn themes_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(d) = TEST_DIR.with(|d| d.borrow().clone()) {
        return d;
    }
    crate::util::bro_dir().join("themes")
}

fn file_of(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.toml"))
}

/// Names of the theme files in the themes dir, sorted.
pub fn custom_names() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "toml").then(|| p.file_stem().map(|s| s.to_string_lossy().to_string()))?
        })
        .collect();
    v.sort();
    v
}

/// True for a theme that comes from a file.
pub fn is_custom(name: &str) -> bool {
    !builtin_names().iter().any(|b| b == name) && file_of(name).is_file()
}

const KEYS: &[&str] = &["accent", "shine", "good", "danger", "inline", "muted", "frame", "user", "text", "fg", "background", "bg", "panel", "sidebar", "cursor"];

fn set_field(t: &mut Theme, key: &str, c: Color) {
    // "terminal" / "none" for the extras: back to none
    let extra = (c != Color::Reset).then_some(c);
    match key {
        "accent" => t.accent = c,
        "shine" => t.shine = c,
        "good" => t.good = c,
        "danger" => t.danger = c,
        "inline" => t.inline = c,
        "muted" => t.muted = c,
        "frame" => t.frame = c,
        "user" => t.user = c,
        "text" | "fg" => t.fg = c,
        "background" | "bg" => t.bg = c,
        "panel" | "sidebar" => t.panel = extra,
        "cursor" => t.cursor = extra,
        _ => {}
    }
}

/// A colour in a theme file: "#rrggbb", "#rgb", an ANSI name ("red", "bright-blue"), or "terminal".
pub fn color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if matches!(s.as_str(), "terminal" | "default" | "none" | "reset") {
        return Some(Color::Reset);
    }
    if let Some(h) = s.strip_prefix('#') {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        return match h.len() {
            3 => hex(&h.chars().flat_map(|c| [c, c]).collect::<String>()),
            6 => hex(h),
            _ => None,
        };
    }
    Some(match s.replace(['-', '_', ' '], "").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" | "gray" | "grey" => Color::Gray,
        "brightblack" | "darkgray" | "darkgrey" => Color::DarkGray,
        "brightred" => Color::LightRed,
        "brightgreen" => Color::LightGreen,
        "brightyellow" => Color::LightYellow,
        "brightblue" => Color::LightBlue,
        "brightmagenta" => Color::LightMagenta,
        "brightcyan" => Color::LightCyan,
        "brightwhite" => Color::White,
        _ => return None,
    })
}

/// The last version of each theme file that parsed, so a typo mid-edit doesn't flash another theme.
static LAST_GOOD: parking_lot::Mutex<Vec<(String, Theme)>> = parking_lot::Mutex::new(Vec::new());

fn custom(name: &str, depth: usize) -> Option<Theme> {
    let text = std::fs::read_to_string(file_of(name)).ok()?;
    let Ok(tbl) = text.parse::<toml::Table>() else {
        let last = LAST_GOOD.lock().iter().find(|(n, _)| n == name).map(|(_, t)| t.clone());
        return Some(last.unwrap_or_else(|| Theme { name: name.to_string(), ..builtin(DEFAULT) }));
    };
    let base = tbl.get("base").and_then(|v| v.as_str()).unwrap_or(DEFAULT);
    let mut t = if base == name || depth >= 4 { builtin(base) } else { get_depth(base, depth + 1) };
    t.name = name.to_string();
    for (k, v) in &tbl {
        if let Some(c) = v.as_str().and_then(color) {
            set_field(&mut t, k, c);
        }
    }
    if let Some(b) = tbl.get("rainbow").and_then(|v| v.as_bool()) {
        t.animated = b;
    }
    if let Some(a) = tbl.get("ansi").and_then(ansi_of) {
        t.ansi = Some(a);
    }
    if let Some(s) = tbl.get("skin").and_then(|v| v.as_str()) {
        t.skin = crate::skin::for_theme(s);
    }
    if let Some(b) = tbl.get("borders").and_then(|v| v.as_str()).and_then(crate::skin::border_named) {
        t.skin.border = b;
        t.skin.focus = b;
    }
    let mut last = LAST_GOOD.lock();
    last.retain(|(n, _)| n != name);
    last.push((name.to_string(), t.clone()));
    Some(t)
}

/// A theme file's `ansi = [16 colours]`.
fn ansi_of(v: &toml::Value) -> Option<[Color; 16]> {
    let v = v.as_array()?;
    let cs: Vec<Color> = v.iter().filter_map(|c| c.as_str().and_then(color)).collect();
    (v.len() == 16 && cs.len() == 16).then(|| std::array::from_fn(|i| cs[i]))
}

/// Anything wrong with a theme file, in words ("mine.toml: 'acent' isn't a setting").
pub fn problems(name: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(file_of(name)) else { return vec![] };
    let file = format!("{name}.toml");
    match text.parse::<toml::Table>() {
        Err(e) => vec![format!("{file}: {}", e.to_string().lines().next().unwrap_or("can't read it"))],
        Ok(tbl) => tbl
            .iter()
            .filter_map(|(k, v)| match k.as_str() {
                "base" => v.as_str().filter(|b| !names().iter().any(|n| n == b)).map(|b| format!("{file}: there's no theme called '{b}'")),
                "rainbow" => (!v.is_bool()).then(|| format!("{file}: rainbow is true or false")),
                "ansi" => ansi_of(v).is_none().then(|| format!("{file}: ansi is a list of 16 colours")),
                "skin" => v.as_str().filter(|s| builtin_names().iter().any(|n| n == s)).is_none().then(|| format!("{file}: there's no theme called '{v}' to borrow a skin from")),
                "borders" => v.as_str().and_then(crate::skin::border_named).is_none().then(|| format!("{file}: borders is rounded, plain, double, thick, dashed, heavy-dashed, ascii or block")),
                k if KEYS.contains(&k) => v.as_str().and_then(color).is_none().then(|| format!("{file}: '{k}' isn't a colour")),
                _ => Some(format!("{file}: '{k}' isn't a setting")),
            })
            .collect(),
    }
}

/// Watch the themes dir; `on_change` runs (on a watcher thread) when a file is written.
pub fn watch(on_change: impl Fn() + Send + 'static) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    if cfg!(test) {
        return None;
    }
    let dir = themes_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| e.kind.is_modify() || e.kind.is_create()) {
            on_change();
        }
    })
    .ok()?;
    w.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_themes_are_complete_and_readable() {
        let names = builtin_names();
        let lum = |c: Color| match c {
            Color::Rgb(r, g, b) => (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) / 255.0,
            _ => 0.0,
        };
        for p in PAINTED {
            assert!(names.iter().any(|n| n == p.name), "{} listed", p.name);
            let t = get(p.name);
            assert_eq!(t.name, p.name);
            assert!(t.painted() && t.ansi.is_some());
            for c in [t.bg, t.fg, t.accent, t.shine, t.frame, t.muted, t.user, t.inline, t.good, t.danger].into_iter().chain(t.ansi.unwrap()) {
                assert!(matches!(c, Color::Rgb(..)), "{}: every colour parses", p.name);
            }
            // body text and hints stand clear of the background
            assert!((lum(t.fg) - lum(t.bg)).abs() > 0.4, "{}: text contrast", p.name);
            assert!((lum(t.muted) - lum(t.bg)).abs() > 0.2, "{}: hint contrast", p.name);
            // the side bar is a shade of the same background, never a typo'd colour from elsewhere
            assert_eq!(t.panel.is_some(), p.panel.is_some(), "{}: panel parses", p.name);
            assert_eq!(t.cursor.is_some(), p.cursor.is_some(), "{}: cursor parses", p.name);
            if let Some(panel) = t.panel {
                assert!((lum(panel) - lum(t.bg)).abs() < 0.08, "{}: panel close to the background", p.name);
            }
        }
        let mut seen = std::collections::HashSet::new();
        assert!(names.iter().all(|n| seen.insert(n)), "no duplicate names");
    }

    #[test]
    fn vscode_favourites_come_painted() {
        for name in [
            "github-dark", "github-dimmed", "github-light", "one-dark-pro", "one-light", "dracula", "monokai-pro", "night-owl", "night-owl-light", "ayu-dark", "ayu-mirage", "ayu-light", "palenight",
            "tokyo-night-storm", "catppuccin-macchiato", "catppuccin-frappe", "solarized-dark", "solarized-light", "andromeda", "aura", "poimandres", "vitesse-dark", "noctis", "synthwave-84",
            "shades-of-purple", "cobalt2", "panda", "dark-modern",
        ] {
            let t = get(name);
            assert_eq!(t.name, name, "{name} is built in");
            assert!(t.painted() && t.ansi.is_some(), "{name} paints its own background and terminal colours");
        }
        for name in ["github-light", "one-light", "night-owl-light", "ayu-light", "solarized-light"] {
            assert!(get(name).is_light(), "{name} draws light");
        }
        // Dracula is the whole official palette now, not just its accents
        assert_eq!(get("dracula").bg, Color::Rgb(0x28, 0x2a, 0x36));
        assert_eq!(get("github-dark").panel, Some(Color::Rgb(0x01, 0x04, 0x09)), "VS Code's side bar");
    }

    #[test]
    fn builtins_and_custom_files() {
        let dir = tempfile::tempdir().unwrap();
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        assert!(get("ultra").animated);
        assert_eq!(get("nope").name, "graphite", "unknown names fall back to the default");
        assert!(!get("graphite").animated && !get("graphite").is_light());
        assert!(get("paper").is_light() && names().contains(&"paper".to_string()));
        assert!(names().contains(&"terminal".to_string()));
        std::fs::write(dir.path().join("mine.toml"), "base = \"ocean\"\naccent = \"#ff0000\"\nrainbow = true\nacent = \"x\"\n").unwrap();
        let t = get("mine");
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert!(t.animated);
        assert!(is_custom("mine"));
        assert_eq!(problems("mine").len(), 1);
        assert_eq!(color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
        let sixteen = (0..16).map(|i| format!("\"#0000{i:02x}\"")).collect::<Vec<_>>().join(", ");
        std::fs::write(dir.path().join("pal.toml"), format!("base = \"nord\"\nansi = [{sixteen}]\n")).unwrap();
        assert_eq!(get("pal").ansi.unwrap()[15], Color::Rgb(0, 0, 15));
        assert!(problems("pal").is_empty(), "{:?}", problems("pal"));
        std::fs::write(dir.path().join("pal.toml"), "ansi = [\"#000000\"]\n").unwrap();
        assert_eq!(problems("pal").len(), 1);
        std::fs::write(dir.path().join("pal.toml"), "base = \"nord\"\nskin = \"cyber\"\nborders = \"ascii\"\n").unwrap();
        let t = get("pal");
        assert!(t.skin.invert_titles && t.skin.border == crate::skin::ASCII, "borrowed skin, own borders");
        assert!(problems("pal").is_empty(), "{:?}", problems("pal"));
        std::fs::write(dir.path().join("pal.toml"), "skin = \"nope\"\nborders = \"wiggly\"\n").unwrap();
        assert_eq!(problems("pal").len(), 2);
        // the desktop app's side bar and cursor: set, or back to none
        std::fs::write(dir.path().join("pal.toml"), "base = \"tokyo-night\"\nsidebar = \"#101010\"\ncursor = \"none\"\n").unwrap();
        let t = get("pal");
        assert_eq!((t.panel, t.cursor), (Some(Color::Rgb(16, 16, 16)), None));
        assert!(problems("pal").is_empty(), "{:?}", problems("pal"));
        // Claude Code's theme names the built-in at the end of the chain (the mod's voice follows it)
        std::fs::write(dir.path().join("deep.toml"), "base = \"pal\"\n").unwrap();
        assert_eq!(builtin_base("deep"), "tokyo-night");
        assert_eq!(builtin_base("nord"), "nord");
        TEST_DIR.with(|d| *d.borrow_mut() = None);
    }

    /// Every theme gives Claude Code its own colours in the theme's hues: Claude's orange on the mascot and the
    /// spinner, never just the accent, and every colour of Claude's readable on the theme's background.
    #[test]
    fn every_theme_dresses_claude_code() {
        use bro_core::claude_theme::{backdrop, contrast, hue, rgb_of};
        for name in builtin_names() {
            let t = get(&name);
            let p = claude_palette(&t);
            let v = bro_core::claude_theme::theme_json(&p);
            assert_eq!(v["bro"]["id"], name.as_str());
            let o = v["overrides"].as_object().unwrap();
            if name == "terminal" {
                assert_eq!(v["base"], "dark-ansi");
                assert!(o.is_empty(), "the terminal's own colours, Claude's own picks");
                continue;
            }
            assert_eq!(v["base"], if t.is_light() { "light" } else { "dark" }, "{name}");
            let bg = backdrop(&p);
            let rgb = |key: &str| rgb_of(o.get(key).and_then(|v| v.as_str()).unwrap_or_else(|| panic!("{name}: {key} missing"))).unwrap();
            let h = hue(rgb("claude"));
            assert!(h <= 40.0 || h >= 350.0, "{name}: the mascot keeps Claude's orange (hue {h})");
            let accent = match t.accent {
                Color::Rgb(r, g, b) => Some([r, g, b]),
                _ => None,
            };
            assert_ne!(Some(rgb("claude")), accent, "{name}: the mascot isn't just the accent");
            for key in ["claude", "permission", "autoAccept", "planMode", "success", "error", "warning"] {
                let ratio = contrast(rgb(key), bg);
                // Claude's orange on a light theme: vivid past 3:1, as Claude's own light theme has it
                let min = if key == "claude" && t.is_light() { 2.95 } else { 4.45 };
                assert!(ratio >= min, "{name}: {key} reads at {ratio:.2}:1");
            }
            assert!(contrast(rgb("inactive"), bg) >= 3.95, "{name}: hints stay legible");
            // a painted theme's own text and selection carry over
            if t.painted() {
                assert!(o.contains_key("text") && o.contains_key("userMessageBackground"), "{name}");
            }
        }
    }

    #[test]
    fn xterm_colours_become_rgb() {
        use bro_core::claude_theme::Ink;
        assert_eq!(ink(Color::Indexed(4)), Ink::Ansi(4));
        assert_eq!(ink(Color::Indexed(196)), Ink::Rgb(255, 0, 0));
        assert_eq!(ink(Color::Indexed(244)), Ink::Rgb(128, 128, 128));
        assert_eq!(ink(Color::Reset), Ink::Terminal);
        assert_eq!(ink(Color::LightCyan), Ink::Ansi(14));
    }

    /// `BRO_THEME_DUMP=<dir> cargo test -p bro-tui dump_claude_themes -- --ignored` writes every built-in theme
    /// as the Claude Code theme bro would, to look them over side by side.
    #[test]
    #[ignore]
    fn dump_claude_themes() {
        let Some(dir) = std::env::var_os("BRO_THEME_DUMP").map(PathBuf::from) else { return };
        std::fs::create_dir_all(&dir).unwrap();
        for name in builtin_names() {
            let v = bro_core::claude_theme::theme_json(&claude_palette(&get(&name)));
            std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&v).unwrap()).unwrap();
        }
    }
}
