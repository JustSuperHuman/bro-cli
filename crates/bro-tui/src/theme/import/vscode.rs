//! VS Code workbench -> T3 roles, ported from T3 Code's vscodeThemeImport.ts (MIT).
//! No includes, extension code, URLs or tokenColors are executed. Bro additionally keeps the 16 ANSI colors.

use super::{RESERVED, derive, id_from_name};
use ratatui::style::Color;
use serde_json::{Map, Value};

type Rgb = [f64; 3];
fn luminance(rgb: Rgb) -> f64 {
    let [r, g, b] = rgb.map(|v| {
        let v = v / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}
fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
#[derive(Clone, Copy)]
struct Rgba {
    rgb: Rgb,
    alpha: f64,
}

fn parse(value: &Value) -> Option<Rgba> {
    let text = value.as_str()?.trim();
    if text.starts_with("color(") {
        let body = text.strip_prefix("color(")?.strip_suffix(')')?.trim();
        let (space, body) = body.split_once(char::is_whitespace)?;
        let (channels, alpha) = body
            .split_once('/')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((body, None));
        let number = |v: &str| {
            // Match JavaScript parseFloat, including its accepted numeric prefix.
            static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let re = NUMBER.get_or_init(|| {
                regex::Regex::new(r"^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?")
                    .unwrap()
            });
            let v = v.trim();
            let number = re.find(v)?.as_str().parse::<f64>().ok()?;
            number.is_finite().then_some(if v.ends_with('%') {
                number / 100.0
            } else {
                number
            })
        };
        let rgb = channels
            .split_whitespace()
            .map(number)
            .collect::<Option<Vec<_>>>()?;
        let rgb: Rgb = rgb.try_into().ok()?;
        let alpha = alpha
            .map(|s| number(s.split('/').next().unwrap_or(s)))
            .unwrap_or(Some(1.0))?
            .clamp(0.0, 1.0);
        let rgb = match space.to_ascii_lowercase().as_str() {
            "srgb" => rgb.map(|v| v * 255.0),
            "display-p3" => {
                let [r, g, b] = rgb.map(|v| {
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                });
                [
                    1.2249401762805 * r - 0.2249401762805 * g,
                    -0.042056961239 * r + 1.042056961239 * g,
                    -0.0196375547643 * r - 0.0786360655012 * g + 1.0982736202656 * b,
                ]
                .map(|v| {
                    let v = v.clamp(0.0, 1.0);
                    (if v <= 0.0031308 {
                        v * 12.92
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    }) * 255.0
                })
            }
            _ => return None,
        };
        return Some(Rgba { rgb, alpha });
    }
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = if hex.len() <= 4 {
        hex.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        hex.into()
    };
    let channel = |i| u8::from_str_radix(&hex[i..i + 2], 16).map(f64::from).ok();
    Some(Rgba {
        rgb: [channel(0)?, channel(2)?, channel(4)?],
        alpha: if hex.len() == 8 {
            channel(6)? / 255.0
        } else {
            1.0
        },
    })
}
fn flatten(c: Rgba, bg: Rgb) -> String {
    derive::hex(std::array::from_fn(|i| {
        c.rgb[i] * c.alpha + bg[i] * (1.0 - c.alpha)
    }))
}
fn humanize(text: &str) -> String {
    if text.chars().any(char::is_whitespace) || !text.contains(['-', '_', '.']) {
        return text.trim().into();
    }
    text.trim()
        .split(['-', '_', '.'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            chars
                .next()
                .unwrap()
                .to_uppercase()
                .chain(chars)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn convert(value: Value, _stem: &str) -> Result<(Value, Option<[Color; 16]>), String> {
    let empty = Map::new();
    let colors = value
        .get("colors")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let pick = |keys: &[&str]| keys.iter().find_map(|key| colors.get(*key).and_then(parse));
    let solid = |bg: Rgb, keys: &[&str]| pick(keys).map(|c| flatten(c, bg));
    let canvas = pick(&["editor.background", "editorPane.background"]).ok_or("That VS Code theme has no editor.background color. Export a self-contained color-theme JSON file; extension includes are not loaded.")?;
    let canvas = canvas.rgb;
    let light = match value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("light" | "hc-light") => true,
        Some("dark" | "hc-black") => false,
        _ => luminance(canvas) >= 0.179,
    };
    let canvas_hex = derive::hex(canvas);
    let raised = solid(canvas, &["editorWidget.background", "dropdown.background"]);
    let apart = |a: &str, b: &str| contrast(derive::rgb(a), derive::rgb(b)) >= 1.1;
    let (accent_color, accent) = [
        "focusBorder",
        "button.background",
        "textLink.foreground",
        "activityBarBadge.background",
        "progressBar.background",
        "badge.background",
    ]
    .iter()
    .filter_map(|key| pick(&[*key]).map(|color| (color, flatten(color, canvas))))
    .find(|(_, hex)| apart(hex, &canvas_hex) && raised.as_ref().is_none_or(|r| apart(hex, r)))
    .unwrap_or_else(|| {
        let hex = [
            if light { "#1b4ed8" } else { "#346bf1" },
            "#ffffff",
            "#000000",
        ]
        .into_iter()
        .find(|c| apart(c, &canvas_hex) && raised.as_ref().is_none_or(|r| apart(c, r)))
        .unwrap_or(if light { "#1b4ed8" } else { "#346bf1" });
        (parse(&Value::String(hex.into())).unwrap(), hex.into())
    });
    // Use the original accent channels here, not the already-composited accent hex.
    let muted_accent = flatten(
        Rgba {
            alpha: 0.2,
            ..accent_color
        },
        canvas,
    );
    // Upstream flattens both inputs to 8-bit hex before deriving the palette.
    let mut full = derive::palette(light, derive::rgb(&canvas_hex), derive::rgb(&muted_accent));
    let d = |role: &str| full[role].as_str().unwrap().to_string();
    let sidebar = solid(canvas, &["sideBar.background", "activityBar.background"])
        .unwrap_or_else(|| d("sidebar"));
    let terminal = solid(canvas, &["terminal.background", "panel.background"])
        .unwrap_or_else(|| d("terminalBackground"));
    let sidebar_rgb = derive::rgb(&sidebar);
    let terminal_rgb = derive::rgb(&terminal);
    let raised = raised.unwrap_or_else(|| d("surfaceRaised"));
    let readable = |surface: &str, fallback: &str, keys: &[&str]| {
        let bg = derive::rgb(surface);
        solid(bg, keys)
            .filter(|c| contrast(derive::rgb(c), bg) >= 4.5)
            .unwrap_or_else(|| {
                if contrast(derive::rgb(fallback), bg) >= 4.5 {
                    fallback.into()
                } else if luminance(bg) < 0.179 {
                    "#ffffff".into()
                } else {
                    "#000000".into()
                }
            })
    };
    let status = |fallback: &str, keys: &[&str]| {
        let base = solid(canvas, keys)
            .filter(|c| apart(c, &canvas_hex))
            .unwrap_or_else(|| fallback.into());
        derive::hex(derive::readable_color(
            derive::rgb(&base),
            derive::rgb(&canvas_hex),
            4.5,
        ))
    };
    let action = solid(canvas, &["button.background"])
        .filter(|c| apart(c, &canvas_hex) && apart(c, &raised))
        .unwrap_or_else(|| accent.clone());
    let mut input = [
        d("input"),
        d("surfaceRaised"),
        if light {
            "#d4d4d8".into()
        } else {
            "#1e1e1e".into()
        },
        "#000000".into(),
        "#ffffff".into(),
        "#808080".into(),
    ]
    .into_iter()
    .find(|c| apart(c, &canvas_hex) && apart(c, &action))
    .unwrap_or_else(|| "#808080".into());
    for key in ["input.background", "input.border"] {
        if let Some(c) =
            solid(canvas, &[key]).filter(|c| apart(c, &canvas_hex) && apart(c, &action))
        {
            input = c;
            break;
        }
    }
    let search = solid(canvas, &["editor.findMatchHighlightBackground"])
        .unwrap_or_else(|| d("searchMatchBackground"));
    let active = solid(canvas, &["editor.findMatchBackground"])
        .unwrap_or_else(|| d("searchMatchActiveBackground"));
    let mut overrides = Map::new();
    let mut set = |role: &str, value: String| {
        overrides.insert(role.into(), Value::String(value));
    };
    set("canvas", canvas_hex.clone());
    set(
        "text",
        readable(
            &canvas_hex,
            &d("text"),
            &["editor.foreground", "foreground"],
        ),
    );
    set(
        "textMuted",
        readable(
            &canvas_hex,
            &d("textMuted"),
            &["descriptionForeground", "disabledForeground"],
        ),
    );
    for (role, keys) in [
        ("surface", &["editorWidget.background"][..]),
        (
            "surfaceOverlay",
            &[
                "menu.background",
                "quickInput.background",
                "dropdown.background",
            ][..],
        ),
        (
            "border",
            &["panel.border", "editorGroup.border", "contrastBorder"][..],
        ),
        (
            "accentSurface",
            &["list.activeSelectionBackground", "list.hoverBackground"][..],
        ),
        ("codeBackground", &["textCodeBlock.background"][..]),
    ] {
        set(role, solid(canvas, keys).unwrap_or_else(|| d(role)));
    }
    set("surfaceRaised", raised.clone());
    set("input", input);
    set(
        "placeholder",
        readable(&raised, &d("placeholder"), &["input.placeholderForeground"]),
    );
    set(
        "error",
        status(&d("error"), &["editorError.foreground", "errorForeground"]),
    );
    set(
        "warning",
        status(&d("warning"), &["editorWarning.foreground"]),
    );
    set(
        "searchMatchForeground",
        readable(
            &search,
            &d("searchMatchForeground"),
            &[
                "editor.findMatchHighlightForeground",
                "editor.foreground",
                "foreground",
            ],
        ),
    );
    set(
        "searchMatchActiveForeground",
        readable(
            &active,
            &d("searchMatchActiveForeground"),
            &[
                "editor.findMatchForeground",
                "editor.foreground",
                "foreground",
            ],
        ),
    );
    set("searchMatchBackground", search);
    set("searchMatchActiveBackground", active);
    set(
        "sidebarForeground",
        readable(&sidebar, &d("sidebarForeground"), &["sideBar.foreground"]),
    );
    set("sidebar", sidebar);
    for (role, keys) in [
        ("sidebarBorder", &["sideBar.border"][..]),
        ("sidebarRowHover", &["list.hoverBackground"][..]),
        (
            "sidebarRowActive",
            &["list.inactiveSelectionBackground", "list.hoverBackground"][..],
        ),
        (
            "sidebarRowSelected",
            &["list.activeSelectionBackground"][..],
        ),
    ] {
        set(role, solid(sidebar_rgb, keys).unwrap_or_else(|| d(role)));
    }
    set(
        "terminalForeground",
        readable(
            &terminal,
            &d("terminalForeground"),
            &["terminal.foreground"],
        ),
    );
    set("terminalBackground", terminal);
    for (role, keys) in [
        (
            "terminalCursor",
            &["terminalCursor.foreground", "editorCursor.foreground"][..],
        ),
        (
            "terminalSelection",
            &["terminal.selectionBackground", "editor.selectionBackground"][..],
        ),
        ("terminalScrollbar", &["scrollbarSlider.background"][..]),
    ] {
        set(role, solid(terminal_rgb, keys).unwrap_or_else(|| d(role)));
    }
    set("accent", accent.clone());
    set("focus", accent.clone());
    set(
        "messageActionForeground",
        readable(
            &action,
            &d("messageActionForeground"),
            &["button.foreground"],
        ),
    );
    set("messageAction", action);
    set(
        "accentForeground",
        readable(&accent, &d("accentForeground"), &["button.foreground"]),
    );
    full.extend(overrides);
    let name = ["displayName", "name"]
        .into_iter()
        .filter_map(|k| value.get(k).and_then(Value::as_str))
        .map(humanize)
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "VS Code theme".into());
    let name = name
        .chars()
        .scan(0, |units, c| {
            *units += c.len_utf16();
            (*units <= 48).then_some(c)
        })
        .collect::<String>();
    let id = id_from_name(&name);
    let id = if RESERVED.contains(&id.as_str()) {
        format!("{id}-vscode")
    } else {
        id
    };
    let mut ansi = super::super::builtin(if light { "github-light" } else { "github-dark" })
        .ansi
        .unwrap();
    for (i, key) in [
        "Black",
        "Red",
        "Green",
        "Yellow",
        "Blue",
        "Magenta",
        "Cyan",
        "White",
        "BrightBlack",
        "BrightRed",
        "BrightGreen",
        "BrightYellow",
        "BrightBlue",
        "BrightMagenta",
        "BrightCyan",
        "BrightWhite",
    ]
    .iter()
    .enumerate()
    {
        if let Some(c) = solid(terminal_rgb, &[&format!("terminal.ansi{key}")]) {
            let [r, g, b] = derive::rgb(&c).map(|v| v.round() as u8);
            ansi[i] = Color::Rgb(r, g, b);
        }
    }
    Ok((
        serde_json::json!({"version":1,"id":id,"name":name,"appearance":if light {"light"} else {"dark"},"colors":full}),
        Some(ansi),
    ))
}
