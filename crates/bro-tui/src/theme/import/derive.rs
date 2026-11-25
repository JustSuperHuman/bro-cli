//! Port of T3 Code's vivid palette, linked editor families and color/contrast helpers.
//!
//! Source: pingdotgg/t3code, commit 5a20d59db38abef8e0b98cfe47fed81d074bcecc.
//! Copyright (c) 2026 T3 Tools Inc., MIT; see assets/themes/LICENSE-t3code.txt.
//! Keep the two transfer-function thresholds, gamut tolerance, solver iteration
//! counts, and intermediate 8-bit rounding: they are observable upstream behavior.

use csscolorparser::Color as CssColor;
use serde_json::{Map, Value};

type Rgb = [f64; 3];

const LIGHT_FOREGROUND: Rgb = [255.0, 250.0, 255.0];
const DARK_FOREGROUND: Rgb = [36.0, 21.0, 35.0];
const WHITE_FOREGROUND: Rgb = [255.0; 3];
const BLACK_FOREGROUND: Rgb = [0.0; 3];

#[derive(Clone, Copy, Debug)]
struct Oklch {
    l: f64,
    c: f64,
    h: f64,
}

fn mix(base: Rgb, overlay: Rgb, amount: f64) -> Rgb {
    std::array::from_fn(|i| base[i] + (overlay[i] - base[i]) * amount)
}

fn luminance(color: Rgb) -> f64 {
    // WCAG's legacy threshold here differs deliberately from the RGB/OKLCH conversion.
    let linearize = |channel: f64| {
        let normalized = channel / 255.0;
        if normalized <= 0.03928 {
            normalized / 12.92
        } else {
            ((normalized + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linearize(color[0]) + 0.7152 * linearize(color[1]) + 0.0722 * linearize(color[2])
}

fn contrast(first: Rgb, second: Rgb) -> f64 {
    let first = luminance(first);
    let second = luminance(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn srgb_channel_to_linear(channel: f64) -> f64 {
    let c = channel / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_channel_to_srgb(channel: f64, round: bool) -> f64 {
    let c = if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    let srgb = c.clamp(0.0, 1.0) * 255.0;
    if round { srgb.round() } else { srgb }
}

fn rgb_to_oklch(color: Rgb) -> Oklch {
    let [r, g, b] = color.map(srgb_channel_to_linear);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let lightness = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    Oklch {
        l: lightness,
        c: a.hypot(bb),
        h: bb.atan2(a) * 180.0 / std::f64::consts::PI,
    }
}

fn oklch_to_linear(color: Oklch) -> Rgb {
    let hr = color.h * std::f64::consts::PI / 180.0;
    let a = color.c * hr.cos();
    let bb = color.c * hr.sin();
    let l = (color.l + 0.3963377774 * a + 0.2158037573 * bb).powi(3);
    let m = (color.l - 0.1055613458 * a - 0.0638541728 * bb).powi(3);
    let s = (color.l - 0.0894841775 * a - 1.291485548 * bb).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ]
}

fn gamut_map(color: Oklch) -> Oklch {
    let in_gamut = |c| {
        oklch_to_linear(Oklch { c, ..color })
            .into_iter()
            .all(|channel| (-0.0001..=1.0001).contains(&channel))
    };
    if in_gamut(color.c) {
        return color;
    }
    let mut low = 0.0;
    let mut high = color.c;
    let resolution: f64 = 0.000001;
    let steps = (color.c.max(resolution).log2() - resolution.log2())
        .ceil()
        .max(1.0) as usize;
    for _ in 0..steps {
        let mid = (low + high) / 2.0;
        if in_gamut(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch { c: low, ..color }
}

fn oklch_to_rgb(color: Oklch, round: bool) -> Rgb {
    oklch_to_linear(gamut_map(color)).map(|channel| linear_channel_to_srgb(channel, round))
}

fn color_number(value: f64, precision: usize) -> String {
    let value = if value.abs() < 10.0_f64.powi(-(precision as i32)) / 2.0 {
        0.0
    } else {
        value
    };
    // JS toFixed chooses the larger magnitude at an exact halfway value;
    // Rust formatting uses ties-to-even. A decimal halfway is also a binary
    // halfway after multiplication by 2^precision (5^precision is odd).
    let bits = value.abs().to_bits();
    let significand = (bits & ((1_u64 << 52) - 1)) | (1_u64 << 52);
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
    let denominator_bits = -(exponent + precision as i32);
    let value =
        if denominator_bits > 0 && significand.trailing_zeros() as i32 == denominator_bits - 1 {
            f64::from_bits(value.to_bits() + 1)
        } else {
            value
        };
    let fixed = format!("{value:.precision$}");
    if fixed.contains('.') {
        fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        fixed
    }
}

fn format_oklch(color: Oklch) -> String {
    format_oklch_alpha(color, 1.0)
}

fn format_oklch_alpha(color: Oklch, alpha: f64) -> String {
    let hue = if color.c < 0.0000005 {
        0.0
    } else {
        ((color.h % 360.0) + 360.0) % 360.0
    };
    let body = format!(
        "{} {} {}",
        color_number(color.l, 6),
        color_number(color.c, 6),
        color_number(hue, 3),
    );
    if alpha < 1.0 {
        format!("oklch({body} / {})", color_number(alpha, 4))
    } else {
        format!("oklch({body})")
    }
}

// Upstream distinguishes these: RGB-derived strings are not gamut-mapped again.
fn rgb_theme_color(color: Rgb) -> String {
    format_oklch(rgb_to_oklch(color))
}

fn theme_color(color: Oklch) -> String {
    format_oklch(gamut_map(color))
}

fn solve_lightness(base: Oklch, against: &[Rgb], minimum: f64, lighter: bool) -> Oklch {
    let contrast_of = |color| {
        let rgb = oklch_to_rgb(color, true);
        against
            .iter()
            .map(|surface| contrast(rgb, *surface))
            .fold(f64::INFINITY, f64::min)
    };
    if contrast_of(base) >= minimum {
        return base;
    }
    let mut low = if lighter { base.l } else { 0.0 };
    let mut high = if lighter { 1.0 } else { base.l };
    for _ in 0..18 {
        let mid = (low + high) / 2.0;
        let candidate = Oklch { l: mid, ..base };
        if contrast_of(candidate) >= minimum {
            if lighter {
                high = mid;
            } else {
                low = mid;
            }
        } else if lighter {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch {
        l: if lighter { high } else { low },
        ..base
    }
}

/// Move only OKLCH lightness, measuring the nearest 8-bit gamut-mapped color.
/// Like T3, an unattainable target returns the endpoint in the chosen direction.
pub(super) fn readable_color(color: Rgb, surface: Rgb, minimum: f64) -> Rgb {
    if contrast(color, surface) >= minimum {
        return color;
    }
    oklch_to_rgb(
        solve_lightness(
            rgb_to_oklch(color),
            &[surface],
            minimum,
            luminance(surface) < 0.179,
        ),
        true,
    )
}

fn parsed_oklch(color: CssColor) -> Oklch {
    // Read the fields, not to_array()/to_linear_rgba(): those clamp to sRGB
    // before we can reduce chroma. This inverts csscolorparser's
    // Color::from_linear_rgba transfer function (used for OKLCH, Lab and P3).
    // The parser stores f32, so this cannot recover Culori's full precision.
    let color = rgb_to_oklch([
        f64::from(color.r) * 255.0,
        f64::from(color.g) * 255.0,
        f64::from(color.b) * 255.0,
    ]);
    Oklch {
        l: color.l.clamp(0.0, 1.0),
        c: color.c.max(0.0),
        ..color
    }
}

/// T3's canonical native spelling, not the gamut-mapped display color.
/// Preserve chroma, clamp L/alpha, and emit L/C to 6 places, hue to 3 and alpha
/// to 4. CSS parsing passes through f32 sRGB in csscolorparser, so wide-gamut
/// values can differ from Culori's direct, double-precision conversion.
pub(super) fn canonical(value: &str) -> Option<String> {
    let (color, alpha) = parse_theme_color(value)?;
    Some(format_oklch_alpha(color, alpha))
}

fn parse_theme_color(value: &str) -> Option<(Oklch, f64)> {
    let input = value.trim();
    let lower = input.to_ascii_lowercase();
    // csscolorparser additionally accepts bare hex, relative colors and calc.
    // Native T3 files accept literal CSS only, as does the enclosing importer.
    if lower
        .split(|c: char| c.is_ascii_whitespace() || c == '(')
        .any(|token| token == "from" || token == "calc")
        || lower.starts_with("hsv(")
        || lower.starts_with("hsva(")
        || lower.starts_with("hwba(")
        || (!input.starts_with('#')
            && !input.contains('(')
            && input.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    let parsed = csscolorparser::parse(input).ok()?;
    if ![parsed.r, parsed.g, parsed.b, parsed.a]
        .into_iter()
        .all(f32::is_finite)
    {
        return None;
    }
    let color = parsed_oklch(parsed);
    if ![color.l, color.c, color.h].into_iter().all(f64::is_finite) {
        return None;
    }
    let none_alpha = lower
        .rsplit_once('/')
        .and_then(|(_, alpha)| alpha.trim().strip_suffix(')'))
        .is_some_and(|alpha| alpha.trim() == "none");
    let alpha = if none_alpha {
        0.0
    } else {
        f64::from(parsed.a).clamp(0.0, 1.0)
    };
    Some((color, alpha))
}

/// A validated literal CSS color, reduced into sRGB and rounded as T3 parses RGB.
/// Alpha is intentionally ignored, just as in parseThemeRgbColor.
pub(super) fn rgb(value: &str) -> Rgb {
    let parsed = csscolorparser::parse(value).expect("the importer validates literal CSS colors");
    oklch_to_rgb(parsed_oklch(parsed), true)
}

/// Map a native color without premature 8-bit rounding, preserving its alpha.
/// Native rendering can then composite against its surface before quantizing.
pub(super) fn srgb(color: CssColor) -> CssColor {
    let [r, g, b] = oklch_to_rgb(parsed_oklch(color), false);
    CssColor::new(
        (r / 255.0) as f32,
        (g / 255.0) as f32,
        (b / 255.0) as f32,
        color.a,
    )
}

pub(super) fn hex(rgb: Rgb) -> String {
    let [r, g, b] = rgb.map(|channel| channel.clamp(0.0, 255.0).round() as u8);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn readable_foreground(background: Rgb) -> Rgb {
    let light_contrast = contrast(background, LIGHT_FOREGROUND);
    let dark_contrast = contrast(background, DARK_FOREGROUND);
    if light_contrast.max(dark_contrast) >= 4.5 {
        return if light_contrast >= dark_contrast {
            LIGHT_FOREGROUND
        } else {
            DARK_FOREGROUND
        };
    }
    if contrast(background, WHITE_FOREGROUND) >= contrast(background, BLACK_FOREGROUND) {
        WHITE_FOREGROUND
    } else {
        BLACK_FOREGROUND
    }
}

fn readable_text(background: Rgb, foreground: Rgb, amount: f64, minimum: f64) -> Rgb {
    let softened = mix(foreground, background, amount);
    if contrast(softened, background) >= minimum {
        return softened;
    }
    let mut readable = foreground;
    let mut lower = 0.0;
    let mut upper = amount;
    for _ in 0..12 {
        let candidate_amount = (lower + upper) / 2.0;
        let candidate = mix(foreground, background, candidate_amount);
        if contrast(candidate, background) >= minimum {
            readable = candidate;
            lower = candidate_amount;
        } else {
            upper = candidate_amount;
        }
    }
    readable
}

fn standard_muted_text(background: Rgb, foreground: Rgb) -> Rgb {
    let target = if luminance(background) < 0.179 {
        5.082
    } else {
        4.705
    };
    readable_text(background, foreground, 1.0, target)
}

fn status_colors(canvas: Rgb) -> [(&'static str, String); 6] {
    let dark = luminance(canvas) < 0.179;
    let (error, error_foreground, warning, warning_foreground) = if dark {
        (
            [251.0, 65.0, 74.0],
            [255.0, 100.0, 103.0],
            [254.0, 154.0, 0.0],
            [255.0, 185.0, 0.0],
        )
    } else {
        (
            [251.0, 44.0, 54.0],
            [193.0, 0.0, 7.0],
            [254.0, 154.0, 0.0],
            [187.0, 77.0, 0.0],
        )
    };
    let surface_mix = if dark { 0.16 } else { 0.08 };
    let error_surface = mix(canvas, error, surface_mix);
    let warning_surface = mix(canvas, warning, surface_mix);
    let readable_on = |foreground, surface| {
        theme_color(solve_lightness(
            rgb_to_oklch(foreground),
            &[surface],
            4.6,
            dark,
        ))
    };
    [
        ("error", rgb_theme_color(error)),
        (
            "errorForeground",
            readable_on(error_foreground, error_surface),
        ),
        ("errorSurface", rgb_theme_color(error_surface)),
        ("warning", rgb_theme_color(warning)),
        (
            "warningForeground",
            readable_on(warning_foreground, warning_surface),
        ),
        ("warningSurface", rgb_theme_color(warning_surface)),
    ]
}

/// Full T3 vivid palette. The requested appearance selects defaults only; the
/// canvas's measured luminance determines all derived light/dark decisions.
pub(super) fn palette(light: bool, canvas_rgb: Rgb, accent_rgb: Rgb) -> Map<String, Value> {
    let mut colors = super::defaults(light);
    let canvas = rgb_to_oklch(canvas_rgb);
    let accent = rgb_to_oklch(accent_rgb);
    let dark = luminance(canvas_rgb) < 0.179;
    let hue = if accent.c < 0.02 { canvas.h } else { accent.h };
    let tint_c = (accent.c * 0.22).clamp(0.008, 0.045);
    let step = if dark { 1.0 } else { -1.0 };
    let surface_at = |delta_l: f64, chroma: f64| Oklch {
        l: (canvas.l + step * delta_l).clamp(0.05, 0.98),
        c: chroma,
        h: hue,
    };
    let text_base = Oklch {
        l: if dark { 0.95 } else { 0.2 },
        c: 0.035_f64.min(accent.c * 0.25),
        h: hue,
    };
    let text = solve_lightness(text_base, &[canvas_rgb], 7.0, dark);
    let text_rgb = oklch_to_rgb(text, true);
    let text_muted_rgb = standard_muted_text(canvas_rgb, text_rgb);
    let action = Oklch {
        l: (accent.l + if dark { 0.06 } else { -0.02 }).clamp(0.35, 0.85),
        c: (accent.c * 0.9).max(0.06),
        h: (hue + 50.0) % 360.0,
    };
    let action_rgb = oklch_to_rgb(action, true);
    let action_foreground = readable_foreground(action_rgb);
    let accent_foreground = readable_foreground(accent_rgb);
    let sidebar = surface_at(0.045, tint_c * 1.4);
    let sidebar_rgb = oklch_to_rgb(sidebar, true);
    let surface = surface_at(0.015, tint_c);
    let surface_raised = surface_at(0.05, tint_c);
    let surface_raised_rgb = oklch_to_rgb(surface_raised, true);
    let surface_overlay = surface_at(0.075, tint_c);
    let border = surface_at(
        if dark { 0.16 } else { 0.12 },
        0.07_f64.min(accent.c * 0.35),
    );
    let input = surface_at(if dark { 0.21 } else { 0.16 }, 0.08_f64.min(accent.c * 0.4));
    let secondary = surface_at(if dark { 0.1 } else { 0.06 }, 0.09_f64.min(accent.c * 0.5));
    let secondary_rgb = oklch_to_rgb(secondary, true);
    let muted = surface_at(
        if dark { 0.06 } else { 0.04 },
        0.06_f64.min(accent.c * 0.35),
    );
    let muted_rgb = oklch_to_rgb(muted, true);
    let accent_surface = surface_at(
        if dark { 0.13 } else { 0.08 },
        0.11_f64.min(accent.c * 0.55),
    );
    let accent_surface_rgb = oklch_to_rgb(accent_surface, true);
    let message_surface = surface_at(if dark { 0.16 } else { 0.1 }, 0.13_f64.min(accent.c * 0.6));
    let message_surface_rgb = oklch_to_rgb(message_surface, true);
    let code_background = surface_at(0.035, tint_c * 0.8);
    let update_surface = surface_at(
        if dark { 0.14 } else { 0.09 },
        0.12_f64.min(accent.c * 0.55),
    );
    let foreground_on =
        |surface_rgb| theme_color(solve_lightness(text_base, &[surface_rgb], 4.6, dark));
    let muted_foreground = rgb_theme_color(readable_text(muted_rgb, text_rgb, 1.0, 4.6));
    let placeholder = rgb_theme_color(readable_text(surface_raised_rgb, text_rgb, 1.0, 4.6));
    let search_match = oklch_to_rgb(surface_at(0.22, 0.12_f64.min(accent.c)), true);
    let search_match_active = oklch_to_rgb(surface_at(0.4, 0.18_f64.min(accent.c)), true);
    let action_hover = Oklch {
        l: action.l + if dark { 0.06 } else { -0.06 },
        ..action
    };

    let derived = [
        ("canvas", rgb_theme_color(canvas_rgb)),
        ("chrome", rgb_theme_color(canvas_rgb)),
        ("toolbar", rgb_theme_color(canvas_rgb)),
        ("toolbarForeground", rgb_theme_color(text_rgb)),
        (
            "toolbarBorder",
            theme_color(surface_at(
                if dark { 0.14 } else { 0.1 },
                0.08_f64.min(accent.c * 0.4),
            )),
        ),
        (
            "toolbarControl",
            theme_color(surface_at(if dark { 0.09 } else { 0.05 }, tint_c * 1.3)),
        ),
        ("toolbarControlForeground", rgb_theme_color(text_rgb)),
        (
            "toolbarControlHover",
            theme_color(surface_at(if dark { 0.14 } else { 0.09 }, tint_c * 1.6)),
        ),
        ("surface", theme_color(surface)),
        ("surfaceRaised", theme_color(surface_raised)),
        ("surfaceOverlay", theme_color(surface_overlay)),
        ("text", rgb_theme_color(text_rgb)),
        ("textMuted", rgb_theme_color(text_muted_rgb)),
        ("border", theme_color(border)),
        ("input", theme_color(input)),
        ("focus", rgb_theme_color(accent_rgb)),
        ("accent", rgb_theme_color(accent_rgb)),
        ("accentForeground", rgb_theme_color(accent_foreground)),
        ("secondary", theme_color(secondary)),
        ("secondaryForeground", foreground_on(secondary_rgb)),
        ("muted", theme_color(muted)),
        ("mutedForeground", muted_foreground),
        ("placeholder", placeholder),
        ("secondaryLabel", rgb_theme_color(text_muted_rgb)),
        ("iconMuted", rgb_theme_color(text_muted_rgb)),
        ("update", rgb_theme_color(accent_rgb)),
        (
            "updateForeground",
            foreground_on(oklch_to_rgb(update_surface, true)),
        ),
        ("updateSurface", theme_color(update_surface)),
        ("accentSurface", theme_color(accent_surface)),
        ("accentSurfaceForeground", foreground_on(accent_surface_rgb)),
        ("messageSurface", theme_color(message_surface)),
        ("messageForeground", foreground_on(message_surface_rgb)),
        ("messageAction", rgb_theme_color(action_rgb)),
        (
            "messageActionForeground",
            rgb_theme_color(action_foreground),
        ),
        ("messageActionHover", theme_color(action_hover)),
        ("codeBackground", theme_color(code_background)),
        ("codeForeground", rgb_theme_color(text_rgb)),
        ("searchMatchBackground", rgb_theme_color(search_match)),
        (
            "searchMatchForeground",
            rgb_theme_color(readable_foreground(search_match)),
        ),
        (
            "searchMatchActiveBackground",
            rgb_theme_color(search_match_active),
        ),
        (
            "searchMatchActiveForeground",
            rgb_theme_color(readable_foreground(search_match_active)),
        ),
        ("sidebar", theme_color(sidebar)),
        ("sidebarForeground", foreground_on(sidebar_rgb)),
        (
            "sidebarMutedForeground",
            rgb_theme_color(standard_muted_text(sidebar_rgb, text_rgb)),
        ),
        (
            "sidebarControlSurface",
            theme_color(surface_at(if dark { 0.1 } else { 0.07 }, tint_c * 1.5)),
        ),
        (
            "sidebarRowHover",
            theme_color(surface_at(
                if dark { 0.08 } else { 0.06 },
                0.08_f64.min(accent.c * 0.45),
            )),
        ),
        (
            "sidebarRowActive",
            theme_color(surface_at(
                if dark { 0.12 } else { 0.09 },
                0.1_f64.min(accent.c * 0.55),
            )),
        ),
        (
            "sidebarRowSelected",
            theme_color(surface_at(
                if dark { 0.14 } else { 0.1 },
                0.11_f64.min(accent.c * 0.6),
            )),
        ),
        (
            "sidebarBorder",
            theme_color(surface_at(
                if dark { 0.17 } else { 0.12 },
                0.08_f64.min(accent.c * 0.4),
            )),
        ),
        ("terminalBackground", rgb_theme_color(canvas_rgb)),
        ("terminalForeground", rgb_theme_color(text_rgb)),
        ("terminalCursor", rgb_theme_color(accent_rgb)),
        (
            "terminalSelection",
            theme_color(surface_at(
                if dark { 0.18 } else { 0.12 },
                0.12_f64.min(accent.c * 0.55),
            )),
        ),
        (
            "terminalScrollbar",
            theme_color(surface_at(if dark { 0.22 } else { 0.16 }, tint_c)),
        ),
        (
            "terminalScrollbarHover",
            theme_color(surface_at(if dark { 0.3 } else { 0.22 }, tint_c)),
        ),
    ];
    colors.extend(
        status_colors(canvas_rgb)
            .into_iter()
            .chain(derived)
            .map(|(role, value)| (role.to_owned(), Value::String(value))),
    );
    colors
}

// Current T3 ThemeEditorPanel.tsx, main 02f8a328 (2026-11-16): flattened
// THEME_EDITOR_ROLE_GROUPS in source order. Each role array includes its
// representative and is also the semantic Inspect/spotlight mapping.
const FAMILIES: &[(&str, &str, &[&str])] = &[
    // Foundation (0..8).
    ("canvas", "Background", &["canvas", "chrome", "toolbar"]),
    ("surface", "Surface", &["surface"]),
    ("surfaceRaised", "Raised surface", &["surfaceRaised"]),
    ("surfaceOverlay", "Overlay", &["surfaceOverlay"]),
    (
        "text",
        "Text",
        &["text", "toolbarForeground", "toolbarControlForeground"],
    ),
    (
        "mutedForeground",
        "Muted text",
        &[
            "textMuted",
            "mutedForeground",
            "placeholder",
            "secondaryLabel",
            "iconMuted",
            "sidebarMutedForeground",
        ],
    ),
    (
        "border",
        "Border",
        &["border", "toolbarBorder", "sidebarBorder"],
    ),
    ("input", "Input", &["input"]),
    // Brand & content (8..14).
    (
        "secondary",
        "Subtle surface",
        &[
            "secondary",
            "secondaryForeground",
            "muted",
            "toolbarControl",
        ],
    ),
    (
        "accentSurface",
        "Highlight surface",
        &[
            "accentSurface",
            "accentSurfaceForeground",
            "toolbarControlHover",
        ],
    ),
    (
        "accent",
        "Accent",
        &[
            "accent",
            "accentForeground",
            "focus",
            "update",
            "updateForeground",
            "updateSurface",
            "terminalCursor",
        ],
    ),
    (
        "messageAction",
        "Action",
        &[
            "messageAction",
            "messageActionForeground",
            "messageActionHover",
        ],
    ),
    (
        "messageSurface",
        "Message surface",
        &["messageSurface", "messageForeground"],
    ),
    (
        "codeBackground",
        "Code surface",
        &["codeBackground", "codeForeground"],
    ),
    // Search (14..16).
    (
        "searchMatchBackground",
        "Search matches",
        &["searchMatchBackground", "searchMatchForeground"],
    ),
    (
        "searchMatchActiveBackground",
        "Current match",
        &["searchMatchActiveBackground", "searchMatchActiveForeground"],
    ),
    // Context (16..20).
    (
        "sidebar",
        "Sidebar background",
        &["sidebar", "sidebarForeground"],
    ),
    (
        "sidebarControlSurface",
        "Sidebar controls",
        &["sidebarControlSurface"],
    ),
    (
        "sidebarRowSelected",
        "Sidebar selection",
        &["sidebarRowHover", "sidebarRowActive", "sidebarRowSelected"],
    ),
    (
        "terminalBackground",
        "Terminal background",
        &[
            "terminalBackground",
            "terminalForeground",
            "terminalSelection",
            "terminalScrollbar",
            "terminalScrollbarHover",
        ],
    ),
    // Status (20..22).
    (
        "error",
        "Error",
        &["error", "errorForeground", "errorSurface"],
    ),
    (
        "warning",
        "Warning",
        &["warning", "warningForeground", "warningSurface"],
    ),
];

/// T3's 22 editor controls: representative role, label, all linked roles.
/// Source groups are Foundation, Brand & content, Search, Context and Status;
/// entries are flattened in that order, and linked-role order is preserved.
pub(super) fn families() -> &'static [(&'static str, &'static str, &'static [&'static str])] {
    FAMILIES
}

/// Update T3's linked roles without re-deriving unrelated palette entries.
/// The editor validates input at this boundary rather than storing upstream's
/// invalid-color passthrough. Valid inputs otherwise follow updateThemeColorFamily.
pub(super) fn update_family(
    light: bool,
    colors: &Map<String, Value>,
    role: &str,
    value: &str,
) -> Result<Map<String, Value>, String> {
    if !super::ROLES.contains(&role) {
        return Err(format!("\"{role}\" is not a supported theme color role."));
    }
    let (selected_color, alpha) = parse_theme_color(value)
        .ok_or_else(|| format!("The color for \"{role}\" must be a literal CSS color."))?;
    let normalized = format_oklch_alpha(selected_color, alpha);
    let color_rgb = |role: &str, fallback: Rgb| {
        colors
            .get(role)
            .and_then(Value::as_str)
            .and_then(parse_theme_color)
            .map(|(color, _)| oklch_to_rgb(color, true))
            .unwrap_or(fallback)
    };
    let canvas = color_rgb(
        "canvas",
        if light {
            [250.0, 245.0, 250.0]
        } else {
            [24.0, 15.0, 27.0]
        },
    );
    let selected = oklch_to_rgb(selected_color, true);
    let selected_on = |background| mix(background, selected, alpha);
    let selected_on_canvas = selected_on(canvas);
    let accent = color_rgb("accent", [168.0, 67.0, 112.0]);
    let canvas_is_dark = luminance(canvas) < 0.179;
    let terminal_is_dark = luminance(selected_on_canvas) < 0.179;
    let foreground_on = |background| rgb_theme_color(readable_foreground(background));
    let selected_tone_on = |background| {
        theme_color(solve_lightness(
            selected_color,
            &[background],
            4.6,
            luminance(background) < 0.179,
        ))
    };
    let mut updated = colors.clone();
    let mut set = |role: &str, value: String| {
        updated.insert(role.to_owned(), Value::String(value));
    };

    match role {
        "canvas" => {
            for role in ["canvas", "chrome", "toolbar"] {
                set(role, normalized.clone());
            }
        }
        "text" => {
            for role in ["text", "toolbarForeground", "toolbarControlForeground"] {
                set(role, normalized.clone());
            }
        }
        "mutedForeground" => {
            for role in [
                "textMuted",
                "mutedForeground",
                "placeholder",
                "secondaryLabel",
                "iconMuted",
                "sidebarMutedForeground",
            ] {
                set(role, normalized.clone());
            }
        }
        "border" => {
            for role in ["border", "toolbarBorder", "sidebarBorder"] {
                set(role, normalized.clone());
            }
        }
        "secondary" => {
            for role in ["secondary", "muted", "toolbarControl"] {
                set(role, normalized.clone());
            }
            set("secondaryForeground", foreground_on(selected_on_canvas));
        }
        "accentSurface" => {
            set("accentSurface", normalized.clone());
            set("accentSurfaceForeground", foreground_on(selected_on_canvas));
            set("toolbarControlHover", normalized);
        }
        "accent" => {
            let update_surface = mix(
                canvas,
                selected_on_canvas,
                if canvas_is_dark { 0.32 } else { 0.16 },
            );
            for role in ["accent", "focus", "update", "terminalCursor"] {
                set(role, normalized.clone());
            }
            set("accentForeground", foreground_on(selected_on_canvas));
            set("updateForeground", selected_tone_on(update_surface));
            set("updateSurface", rgb_theme_color(update_surface));
        }
        "messageAction" => {
            let action_foreground = readable_foreground(selected_on_canvas);
            let toward_opposite =
                if action_foreground == LIGHT_FOREGROUND || action_foreground == WHITE_FOREGROUND {
                    BLACK_FOREGROUND
                } else {
                    WHITE_FOREGROUND
                };
            let action_hover = mix(selected, toward_opposite, 0.12);
            set("messageAction", normalized);
            set(
                "messageActionForeground",
                rgb_theme_color(action_foreground),
            );
            set(
                "messageActionHover",
                format_oklch_alpha(rgb_to_oklch(action_hover), alpha),
            );
        }
        "messageSurface" => {
            set("messageSurface", normalized);
            set("messageForeground", foreground_on(selected_on_canvas));
        }
        "searchMatchBackground" => {
            // Search highlights become opaque after compositing over the canvas.
            set("searchMatchBackground", rgb_theme_color(selected_on_canvas));
            set("searchMatchForeground", foreground_on(selected_on_canvas));
        }
        "searchMatchActiveBackground" => {
            set(
                "searchMatchActiveBackground",
                rgb_theme_color(selected_on_canvas),
            );
            set(
                "searchMatchActiveForeground",
                foreground_on(selected_on_canvas),
            );
        }
        "codeBackground" => {
            set("codeBackground", normalized);
            set("codeForeground", foreground_on(selected_on_canvas));
        }
        "sidebar" => {
            set("sidebar", normalized);
            set("sidebarForeground", foreground_on(selected_on_canvas));
        }
        "sidebarRowSelected" => {
            let sidebar = color_rgb("sidebar", canvas);
            let selected_on_sidebar = selected_on(sidebar);
            set(
                "sidebarRowHover",
                rgb_theme_color(mix(sidebar, selected_on_sidebar, 0.5)),
            );
            set(
                "sidebarRowActive",
                rgb_theme_color(mix(sidebar, selected_on_sidebar, 0.8)),
            );
            set("sidebarRowSelected", normalized);
        }
        "terminalBackground" => {
            let terminal_foreground = readable_foreground(selected_on_canvas);
            set("terminalBackground", normalized);
            set("terminalForeground", rgb_theme_color(terminal_foreground));
            set(
                "terminalSelection",
                rgb_theme_color(mix(
                    selected_on_canvas,
                    accent,
                    if terminal_is_dark { 0.35 } else { 0.18 },
                )),
            );
            set(
                "terminalScrollbar",
                rgb_theme_color(mix(
                    selected_on_canvas,
                    terminal_foreground,
                    if terminal_is_dark { 0.42 } else { 0.22 },
                )),
            );
            set(
                "terminalScrollbarHover",
                rgb_theme_color(mix(
                    selected_on_canvas,
                    terminal_foreground,
                    if terminal_is_dark { 0.55 } else { 0.32 },
                )),
            );
        }
        "error" | "warning" => {
            let surface = mix(
                canvas,
                selected_on_canvas,
                if canvas_is_dark { 0.16 } else { 0.08 },
            );
            set(role, normalized);
            set(&format!("{role}Foreground"), selected_tone_on(surface));
            set(&format!("{role}Surface"), rgb_theme_color(surface));
        }
        // The five standalone surfaces and all other roles only update themselves.
        _ => set(role, normalized),
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_literal_colors_round_trip() {
        for value in [
            "#000000", "#ffffff", "#fb414a", "#fe9a00", "#a84370", "#180f1b",
        ] {
            assert_eq!(hex(rgb(value)), value);
        }
        assert_eq!(hex([-1.0, 127.5, 256.0]), "#0080ff");
        assert_eq!(rgb("rgb(20 30 40 / 50%)"), [20.0, 30.0, 40.0]);
        assert_eq!(rgb("rebeccapurple"), [102.0, 51.0, 153.0]);
    }

    #[test]
    fn gamut_mapping_reduces_only_chroma_to_the_upstream_tolerance() {
        let color = Oklch {
            l: 0.65,
            c: 0.4,
            h: 40.0,
        };
        let mapped = gamut_map(color);
        assert_eq!(mapped.l, color.l);
        assert_eq!(mapped.h, color.h);
        assert!(mapped.c > 0.0 && mapped.c < color.c);
        assert!(
            oklch_to_linear(mapped)
                .into_iter()
                .all(|v| (-0.0001..=1.0001).contains(&v))
        );
        assert!(
            oklch_to_linear(Oklch {
                c: mapped.c + 0.000001,
                ..mapped
            })
            .into_iter()
            .any(|v| !(-0.0001..=1.0001).contains(&v))
        );
    }

    #[test]
    fn native_mapping_preserves_alpha_and_does_not_clip_channels_first() {
        let input = csscolorparser::parse("oklch(.65 .4 40 / .3)").unwrap();
        let output = srgb(input);
        assert_eq!(output.a, input.a);
        let mapped = rgb_to_oklch([
            f64::from(output.r) * 255.0,
            f64::from(output.g) * 255.0,
            f64::from(output.b) * 255.0,
        ]);
        assert!((mapped.l - 0.65).abs() < 0.0001);
        assert!((mapped.h - 40.0).abs() < 0.1);
        assert!(mapped.c < 0.4);
        let white = srgb(csscolorparser::parse("oklch(1.4 .3 30 / .2)").unwrap());
        assert!(white.r > 0.99 && white.g > 0.99 && white.b > 0.99);
        assert_eq!(white.a, 0.2);
    }

    #[test]
    fn canonical_native_spelling_preserves_chroma_and_formats_alpha() {
        for (input, expected) in [
            ("oklch(.6 .2 280)", "oklch(0.6 0.2 280)"),
            ("oklch(.65 .4 40 / .3)", "oklch(0.65 0.4 40 / 0.3)"),
            ("oklch(.5 .1 50 / .123456)", "oklch(0.5 0.1 50 / 0.1235)"),
            ("OKLCH(50% .1 50 / NONE)", "oklch(0.5 0.1 50 / 0)"),
            ("oklch(.5 .1 50 / 2)", "oklch(0.5 0.1 50)"),
            ("transparent", "oklch(0 0 0 / 0)"),
            ("white", "oklch(1 0 0)"),
        ] {
            assert_eq!(canonical(input).as_deref(), Some(expected), "{input}");
            assert_eq!(canonical(expected).as_deref(), Some(expected), "{expected}");
        }
        assert_eq!(
            canonical("#12345678").as_deref(),
            Some("oklch(0.319168 0.072455 251.168 / 0.4706)")
        );
    }

    #[test]
    fn canonical_rejects_nonliteral_or_non_css_values() {
        for input in [
            "",
            "123456",
            "currentColor",
            "var(--accent)",
            "hsv(0 100% 100%)",
            "hsva(0 100% 100% / 1)",
            "hwba(0 0% 0% / 1)",
            "rgb(from red r g b)",
            "rgb(from\\tred r g b)",
            "rgb(calc(1 + 1) 0 0)",
            "oklch(NaN .1 50)",
        ] {
            assert!(canonical(input).is_none(), "{input}");
        }
    }

    #[test]
    fn readability_returns_the_measured_eight_bit_color() {
        for surface in [[24.0, 15.0, 27.0], [250.0, 245.0, 250.0]] {
            for color in [
                [100.0, 60.0, 80.0],
                [110.0, 110.0, 110.0],
                [240.0, 160.0, 200.0],
            ] {
                let solved = readable_color(color, surface, 4.6);
                assert!(solved.into_iter().all(|v| v == v.round()));
                assert!(
                    contrast(solved, surface) >= 4.6,
                    "{color:?} on {surface:?}: {solved:?}"
                );
                assert_eq!(rgb(&hex(solved)), solved);
                assert_eq!(readable_color(solved, surface, 4.6), solved);
            }
        }
    }

    #[test]
    fn palette_has_all_roles_and_uses_measured_canvas_not_appearance() {
        let accent = [168.0, 67.0, 112.0];
        for canvas in [[24.0, 15.0, 27.0], [250.0, 245.0, 250.0], [120.0; 3]] {
            let light = palette(true, canvas, accent);
            let dark = palette(false, canvas, accent);
            assert_eq!(light, dark);
            assert_eq!(light.len(), 61);
            assert_eq!(
                light.keys().collect::<Vec<_>>(),
                super::super::defaults(true).keys().collect::<Vec<_>>()
            );
            for value in light.values() {
                assert!(
                    csscolorparser::parse(value.as_str().unwrap()).is_ok(),
                    "{value}"
                );
            }
            assert_eq!(rgb(light["canvas"].as_str().unwrap()), canvas);
            assert_eq!(rgb(light["accent"].as_str().unwrap()), accent);
        }
    }

    #[test]
    fn new_seed_colors_regenerate_the_full_vivid_palette() {
        let canvas = [24.0, 15.0, 27.0];
        let accent = [168.0, 67.0, 112.0];
        let original = palette(false, canvas, accent);
        let new_canvas = palette(false, [48.0, 64.0, 80.0], accent);
        let new_accent = palette(false, canvas, [44.0, 120.0, 210.0]);
        for regenerated in [&new_canvas, &new_accent] {
            assert_eq!(regenerated.len(), 61);
            assert_eq!(
                regenerated.keys().collect::<Vec<_>>(),
                original.keys().collect::<Vec<_>>()
            );
            for role in ["surface", "border", "sidebar"] {
                assert_ne!(regenerated[role], original[role], "{role}");
            }
        }
        // For a chromatic accent, the action depends on that accent and the
        // canvas's light/dark classification, not on the canvas's exact RGB.
        // Both canvas seeds here are dark, so only the accent edit changes it.
        assert_eq!(new_canvas["messageAction"], original["messageAction"]);
        assert_ne!(new_accent["messageAction"], original["messageAction"]);
        assert_eq!(new_accent["canvas"], original["canvas"]);
        assert_eq!(new_canvas["accent"], original["accent"]);
        for role in ["chrome", "toolbar", "terminalBackground"] {
            assert_eq!(new_canvas[role], new_canvas["canvas"]);
        }
        for role in ["focus", "update", "terminalCursor"] {
            assert_eq!(new_accent[role], new_accent["accent"]);
        }
    }

    fn assert_only_changed(
        before: &Map<String, Value>,
        after: &Map<String, Value>,
        allowed: &[&str],
    ) {
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>()
        );
        for (role, value) in before {
            if !allowed.contains(&role.as_str()) {
                assert_eq!(&after[role], value, "unrelated role {role}");
            }
        }
    }

    #[test]
    fn editor_families_cover_each_role_once_and_never_mutate_other_families() {
        assert_eq!(families().len(), 22);
        assert_eq!(families()[0].0, "canvas");
        assert_eq!(families()[8].0, "secondary");
        assert_eq!(families()[14].0, "searchMatchBackground");
        assert_eq!(families()[16].0, "sidebar");
        assert_eq!(families()[20].0, "error");
        let colors = palette(false, [24.0, 15.0, 27.0], [168.0, 67.0, 112.0]);
        let mut covered = std::collections::BTreeSet::new();
        for &(role, label, linked) in families() {
            assert!(!label.is_empty());
            assert!(linked.contains(&role), "{role}");
            let updated = update_family(false, &colors, role, "#55aacc80").unwrap();
            assert_only_changed(&colors, &updated, linked);
            for &linked_role in linked {
                assert!(covered.insert(linked_role), "duplicate role {linked_role}");
                assert!(csscolorparser::parse(updated[linked_role].as_str().unwrap()).is_ok());
            }
        }
        assert_eq!(
            covered,
            super::super::ROLES
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[test]
    fn canvas_family_is_not_a_seed_regeneration() {
        let colors = palette(false, [24.0, 15.0, 27.0], [168.0, 67.0, 112.0]);
        let updated = update_family(false, &colors, "canvas", "#ddeeff").unwrap();
        assert_only_changed(&colors, &updated, &["canvas", "chrome", "toolbar"]);
        for role in ["canvas", "chrome", "toolbar"] {
            assert_eq!(updated[role].as_str(), canonical("#ddeeff").as_deref());
        }
    }

    #[test]
    fn family_updates_linked_foregrounds_and_preserves_unrelated_colors() {
        let colors = palette(false, [24.0, 15.0, 27.0], [168.0, 67.0, 112.0]);
        for (role, foreground, linked) in [
            (
                "secondary",
                "secondaryForeground",
                &[
                    "secondary",
                    "secondaryForeground",
                    "muted",
                    "toolbarControl",
                ][..],
            ),
            (
                "accentSurface",
                "accentSurfaceForeground",
                &[
                    "accentSurface",
                    "accentSurfaceForeground",
                    "toolbarControlHover",
                ][..],
            ),
            (
                "messageSurface",
                "messageForeground",
                &["messageSurface", "messageForeground"][..],
            ),
            (
                "codeBackground",
                "codeForeground",
                &["codeBackground", "codeForeground"][..],
            ),
            (
                "sidebar",
                "sidebarForeground",
                &["sidebar", "sidebarForeground"][..],
            ),
        ] {
            let updated = update_family(false, &colors, role, "#efcc80").unwrap();
            assert_only_changed(&colors, &updated, linked);
            assert_eq!(updated[role].as_str(), canonical("#efcc80").as_deref());
            assert_eq!(rgb(updated[foreground].as_str().unwrap()), DARK_FOREGROUND);
            assert!(contrast(rgb("#efcc80"), rgb(updated[foreground].as_str().unwrap())) >= 4.5);
        }
        let updated = update_family(false, &colors, "accent", "#56c5a780").unwrap();
        assert_only_changed(
            &colors,
            &updated,
            &[
                "accent",
                "accentForeground",
                "focus",
                "update",
                "updateForeground",
                "updateSurface",
                "terminalCursor",
            ],
        );
        for role in ["focus", "update", "terminalCursor"] {
            assert_eq!(updated[role], updated["accent"]);
        }
        assert_eq!(
            updated["accent"].as_str(),
            canonical("#56c5a780").as_deref()
        );
        assert!(
            contrast(
                rgb(updated["updateForeground"].as_str().unwrap()),
                rgb(updated["updateSurface"].as_str().unwrap())
            ) >= 4.5
        );
    }

    #[test]
    fn translucent_family_members_use_the_correct_backdrop() {
        let mut colors = palette(false, [0.0; 3], [168.0, 67.0, 112.0]);
        colors.insert("sidebar".into(), Value::String("#204060".into()));
        let sidebar = update_family(false, &colors, "sidebarRowSelected", "#ff000080").unwrap();
        assert_only_changed(
            &colors,
            &sidebar,
            &["sidebarRowSelected", "sidebarRowHover", "sidebarRowActive"],
        );
        assert_eq!(
            rgb(sidebar["sidebarRowHover"].as_str().unwrap()),
            [88.0, 48.0, 72.0]
        );
        assert_eq!(
            rgb(sidebar["sidebarRowActive"].as_str().unwrap()),
            [122.0, 38.0, 57.0]
        );
        assert_eq!(
            sidebar["sidebarRowSelected"].as_str(),
            canonical("#ff000080").as_deref()
        );

        let search = update_family(false, &colors, "searchMatchBackground", "#ffffff80").unwrap();
        assert_only_changed(
            &colors,
            &search,
            &["searchMatchBackground", "searchMatchForeground"],
        );
        assert_eq!(
            rgb(search["searchMatchBackground"].as_str().unwrap()),
            [128.0; 3]
        );
        assert!(
            !search["searchMatchBackground"]
                .as_str()
                .unwrap()
                .contains('/')
        );
        // At #808080 neither tinted foreground reaches 4.5:1; T3 falls
        // back to pure black (5.32:1), not the tinted dark foreground (4.41:1).
        let search_background = [128.0; 3];
        assert!(contrast(search_background, LIGHT_FOREGROUND) < 4.5);
        assert!(contrast(search_background, DARK_FOREGROUND) < 4.5);
        assert!(
            contrast(search_background, BLACK_FOREGROUND)
                > contrast(search_background, WHITE_FOREGROUND)
        );
        assert_eq!(
            rgb(search["searchMatchForeground"].as_str().unwrap()),
            BLACK_FOREGROUND
        );

        let action = update_family(false, &colors, "messageAction", "#ff000080").unwrap();
        assert_only_changed(
            &colors,
            &action,
            &[
                "messageAction",
                "messageActionForeground",
                "messageActionHover",
            ],
        );
        // Light text on the composited dark red means hover moves raw red toward black.
        assert_eq!(
            rgb(action["messageActionHover"].as_str().unwrap()),
            [224.0, 0.0, 0.0]
        );
        assert!(
            action["messageActionHover"]
                .as_str()
                .unwrap()
                .ends_with(" / 0.502)")
        );
    }

    #[test]
    fn status_and_terminal_families_rederive_only_their_linked_roles() {
        let colors = palette(false, [24.0, 15.0, 27.0], [168.0, 67.0, 112.0]);
        for role in ["error", "warning"] {
            let updated = update_family(false, &colors, role, "#9869db").unwrap();
            let foreground = format!("{role}Foreground");
            let surface = format!("{role}Surface");
            assert_only_changed(&colors, &updated, &[role, &foreground, &surface]);
            assert!(
                contrast(
                    rgb(updated[&foreground].as_str().unwrap()),
                    rgb(updated[&surface].as_str().unwrap())
                ) >= 4.5
            );
        }
        let terminal = update_family(false, &colors, "terminalBackground", "#eeeeee").unwrap();
        assert_only_changed(
            &colors,
            &terminal,
            &[
                "terminalBackground",
                "terminalForeground",
                "terminalSelection",
                "terminalScrollbar",
                "terminalScrollbarHover",
            ],
        );
        assert_eq!(
            rgb(terminal["terminalForeground"].as_str().unwrap()),
            DARK_FOREGROUND
        );
        // The selected terminal background, not the old dark canvas, chooses light mixes.
        assert_eq!(
            terminal["terminalSelection"],
            rgb_theme_color(mix([238.0; 3], [168.0, 67.0, 112.0], 0.18))
        );
        assert_eq!(
            terminal["terminalScrollbar"],
            rgb_theme_color(mix([238.0; 3], DARK_FOREGROUND, 0.22))
        );
        assert_eq!(
            terminal["terminalScrollbarHover"],
            rgb_theme_color(mix([238.0; 3], DARK_FOREGROUND, 0.32))
        );
    }

    #[test]
    fn standalone_family_and_invalid_input_are_isolated() {
        let colors = palette(false, [24.0, 15.0, 27.0], [168.0, 67.0, 112.0]);
        let updated = update_family(false, &colors, "surfaceRaised", "#987654").unwrap();
        assert_only_changed(&colors, &updated, &["surfaceRaised"]);
        assert_eq!(
            updated["surfaceRaised"].as_str(),
            canonical("#987654").as_deref()
        );
        // Inspect resolves linked roles to their representative in the UI;
        // direct raw-role edits still use upstream's single-role fallback.
        let raw = update_family(false, &colors, "textMuted", "#987654").unwrap();
        assert_only_changed(&colors, &raw, &["textMuted"]);
        assert_eq!(raw["textMuted"].as_str(), canonical("#987654").as_deref());
        assert!(update_family(false, &colors, "canvas", "var(--background)").is_err());
        assert!(update_family(false, &colors, "unknown", "#ffffff").is_err());
    }

    #[test]
    fn foreground_and_formatting_follow_t3_special_cases() {
        assert_eq!(readable_foreground(BLACK_FOREGROUND), LIGHT_FOREGROUND);
        assert_eq!(readable_foreground(WHITE_FOREGROUND), DARK_FOREGROUND);
        // Neither tinted foreground reaches 4.5 on this background: use pure black/white.
        let mid = [117.0; 3];
        assert!(contrast(mid, LIGHT_FOREGROUND).max(contrast(mid, DARK_FOREGROUND)) < 4.5);
        assert_eq!(readable_foreground(mid), WHITE_FOREGROUND);
        assert_eq!(
            format_oklch(Oklch {
                l: 0.5,
                c: 0.0,
                h: -45.0
            }),
            "oklch(0.5 0 0)"
        );
        assert_eq!(
            format_oklch(Oklch {
                l: 0.5,
                c: 0.2,
                h: -45.0
            }),
            "oklch(0.5 0.2 315)"
        );
        assert_eq!(color_number(-0.00000001, 6), "0");
        assert_eq!(color_number(22.0625, 3), "22.063");
        assert_eq!(color_number(-22.0625, 3), "-22.063");
        assert_eq!(color_number(1.005, 2), "1");
    }
}
