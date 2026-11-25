//! T3 Code v1 theme interchange. Palette data is portable; browser-only CSS is retained, never executed.
//! Contract pinned to pingdotgg/t3code 5a20d59db38abef8e0b98cfe47fed81d074bcecc.

use super::{Theme, builtin};
use csscolorparser::Color as CssColor;
use ratatui::style::Color;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

mod derive;
mod vscode;

pub const ROLES: &[&str] = &[
    "canvas",
    "chrome",
    "toolbar",
    "toolbarForeground",
    "toolbarBorder",
    "toolbarControl",
    "toolbarControlForeground",
    "toolbarControlHover",
    "surface",
    "surfaceRaised",
    "surfaceOverlay",
    "text",
    "textMuted",
    "border",
    "input",
    "focus",
    "accent",
    "accentForeground",
    "secondary",
    "secondaryForeground",
    "muted",
    "mutedForeground",
    "placeholder",
    "secondaryLabel",
    "iconMuted",
    "error",
    "errorForeground",
    "errorSurface",
    "warning",
    "warningForeground",
    "warningSurface",
    "update",
    "updateForeground",
    "updateSurface",
    "accentSurface",
    "accentSurfaceForeground",
    "messageSurface",
    "messageForeground",
    "messageAction",
    "messageActionForeground",
    "messageActionHover",
    "codeBackground",
    "codeForeground",
    "searchMatchBackground",
    "searchMatchForeground",
    "searchMatchActiveBackground",
    "searchMatchActiveForeground",
    "sidebar",
    "sidebarForeground",
    "sidebarMutedForeground",
    "sidebarControlSurface",
    "sidebarRowHover",
    "sidebarRowActive",
    "sidebarRowSelected",
    "sidebarBorder",
    "terminalBackground",
    "terminalForeground",
    "terminalCursor",
    "terminalSelection",
    "terminalScrollbar",
    "terminalScrollbarHover",
];
const RESERVED: &[&str] = &[
    "system",
    "light",
    "dark",
    "t3-chat",
    "grove",
    "ocean",
    "ember",
    "iris",
    "t3-chat-dark",
    "t3-grove",
    "t3-ocean",
    "t3-ember",
    "t3-iris",
];

#[derive(Clone, Debug)]
pub struct Imported {
    /// A validated, complete native file, including metadata, CSS and both appearances.
    pub file: Value,
    pub light: bool,
    pub label: String,
    pub colors: BTreeMap<String, CssColor>,
    /// VS Code's terminal palette is additional data, not a native T3 color role.
    pub ansi: Option<[Color; 16]>,
    /// The CSS is the bundled Aurora stylesheet, which the window draws natively (see `Theme::aurora`). Worked
    /// out once here: the window asks for every card and message it draws, every frame.
    pub aurora: bool,
}

/// The CSS is the bundled Aurora stylesheet.
fn is_aurora_css(css: &str) -> bool {
    css.lines().eq(include_str!("../../assets/themes/aurora-studio.css").lines())
}

/// T3's deterministic two-seed palette, shared by file conversion and the native editor.
pub fn generate(light: bool, canvas: &str, accent: &str) -> Result<Map<String, Value>, String> {
    for value in [canvas, accent] {
        if css(value).is_none() { return Err("Enter a literal color, such as #203040 or oklch(0.6 0.1 250).".into()); }
    }
    Ok(derive::palette(light, derive::rgb(canvas), derive::rgb(accent)))
}

pub fn families() -> &'static [(&'static str, &'static str, &'static [&'static str])] { derive::families() }

pub fn update_family(light: bool, colors: &Map<String, Value>, role: &str, value: &str) -> Result<Map<String, Value>, String> {
    derive::update_family(light, colors, role, value)
}

pub fn display_hex(value: &str) -> Option<String> {
    let parsed=css(value)?;
    let mut hex=derive::hex(derive::rgb(value));
    if parsed.a<1.0 {hex.push_str(&format!("{:02x}",(parsed.a*255.0).round()as u8));}
    Some(hex)
}

pub fn defaults(light: bool) -> Map<String, Value> {
    serde_json::from_str(if light {
        include_str!("../../assets/themes/t3-default-light.json")
    } else {
        include_str!("../../assets/themes/t3-default-dark.json")
    })
    .expect("bundled T3 palettes are valid JSON")
}

fn label(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.encode_utf16().count() <= 48)
}
fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// The same stable ASCII slug / UTF-16 FNV-1a fallback as T3's themeIdFromName.
pub fn id_from_name(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let mut id = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_end_matches('-');
    if !id.is_empty() {
        return id.chars().take(48).collect();
    }
    if lower.is_empty() {
        return "custom-theme".into();
    }
    let mut hash = 2_166_136_261u32;
    for c in lower.encode_utf16() {
        hash = (hash ^ u32::from(c)).wrapping_mul(16_777_619);
    }
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(hash % 36, 36).unwrap());
        hash /= 36;
        if hash == 0 {
            break;
        }
    }
    format!("theme-{}", digits.iter().rev().collect::<String>())
}

/// Parse literal colors only. T3 does not resolve CSS variables or browser context.
pub fn css(value: &str) -> Option<CssColor> {
    let input = value.trim();
    // csscolorparser also accepts bare hexadecimal and nonstandard / relative functions, unlike Culori.
    let lower = input.to_ascii_lowercase();
    if lower.contains("from ")
        || lower.contains("calc(")
        || lower.starts_with("hsv(")
        || lower.starts_with("hsva(")
        || lower.starts_with("hwba(")
    {
        return None;
    }
    if !input.starts_with('#')
        && !input.contains('(')
        && input.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    let mut color = csscolorparser::parse(input).ok()?;
    if !color.to_array().iter().all(|v| v.is_finite()) {
        return None;
    }
    if lower
        .rsplit_once('/')
        .is_some_and(|(_, a)| a.trim().trim_end_matches(')').trim() == "none")
    {
        color.a = 0.0;
    }
    color.a = color.a.clamp(0.0, 1.0);
    Some(derive::srgb(color))
}

fn colors(value: &Value, light: bool) -> Result<Map<String, Value>, String> {
    let overrides = value.as_object().ok_or("Theme colors must be objects.")?;
    if overrides.is_empty() {
        return Err("Add at least one color role to the theme file.".into());
    }
    let mut full = defaults(light);
    for (role, value) in overrides {
        if !ROLES.contains(&role.as_str()) {
            return Err(format!("\"{role}\" is not a supported theme color role."));
        }
        let canonical = value
            .as_str()
            .and_then(derive::canonical)
            .ok_or_else(|| format!("The color for \"{role}\" must be a literal CSS color."))?;
        full.insert(role.clone(), Value::String(canonical));
    }
    Ok(full)
}

fn jsonc(text: &str) -> Result<Value, String> {
    jsonc_parser::parse_to_serde_value(
        text,
        &jsonc_parser::ParseOptions {
            allow_comments: true,
            allow_trailing_commas: true,
            allow_loose_object_property_names: false,
            allow_missing_commas: false,
            allow_single_quoted_strings: false,
            allow_hexadecimal_numbers: false,
            allow_unary_plus_numbers: false,
            allow_bare_decimal_point_numbers: false,
            allow_non_finite_numbers: false,
            allow_extended_string_escapes: false,
        },
    )
    .map_err(|e| format!("Cannot read theme JSON: {e}"))
}

/// Native JSON is strict. JSONC is accepted only for VS Code color-theme files, as it is upstream.
pub fn parse(text: &str, stem: &str, variant: Option<&str>) -> Result<Imported, String> {
    let loose = jsonc(text)?;
    let object = loose
        .as_object()
        .ok_or("Theme files must contain a JSON object.")?;
    let vscode = object.get("version").and_then(Value::as_f64) != Some(1.0)
        && (object
            .get("colors")
            .and_then(Value::as_object)
            .is_some_and(|m| m.keys().any(|k| k.contains('.')))
            || object.get("tokenColors").is_some_and(Value::is_array));
    let (mut file, ansi) = if vscode {
        vscode::convert(loose, stem)?
    } else {
        let strict = serde_json::from_str(text)
            .map_err(|e| format!("T3 theme files require strict JSON: {e}"))?;
        (strict, None)
    };
    let object = file
        .as_object_mut()
        .ok_or("Theme files must contain a JSON object.")?;
    if object.get("version").and_then(Value::as_f64) != Some(1.0) {
        return Err("Unsupported theme version. Expected 1.".into());
    }
    let name = object
        .get("name")
        .and_then(label)
        .ok_or("Theme files need a name (48 characters or fewer).")?
        .to_string();
    let light = match object.get("appearance").and_then(Value::as_str) {
        Some("light") => true,
        Some("dark") => false,
        _ => return Err("Theme files need an appearance of light or dark.".into()),
    };
    let id = match object.get("id") {
        None => id_from_name(&name),
        Some(v) => v.as_str().filter(|id| valid_id(id)).ok_or("Theme ids may only contain lowercase letters, numbers, and hyphens (48 characters or fewer).")?.to_string(),
    };
    if RESERVED.contains(&id.as_str()) {
        return Err(format!("The theme id \"{id}\" is reserved."));
    }
    if let Some(v) = object.get("collection") {
        let c = v
            .as_object()
            .ok_or("Theme collections need a valid id and label.")?;
        let id = c
            .get("id")
            .and_then(Value::as_str)
            .ok_or("Theme collections need a valid id and label.")?;
        if id.is_empty()
            || id.len() > 128
            || !id.as_bytes()[0].is_ascii_alphanumeric()
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".:-".contains(&c))
            || c.get("label").and_then(label).is_none()
        {
            return Err("Theme collections need a valid id and label.".into());
        }
    }
    if let Some(v) = object.get("css")
        && !v
            .as_str()
            .is_some_and(|s| s.encode_utf16().count() <= 50_000)
    {
        return Err("Theme CSS must be text of 50000 characters or fewer.".into());
    }
    let base = colors(
        object
            .get("colors")
            .ok_or("Theme files need a colors object.")?,
        light,
    )?;
    let mut variants = Map::new();
    if let Some(v) = object.get("variants") {
        for (appearance, overrides) in v.as_object().ok_or("Theme variants must be an object.")? {
            let variant_light = match appearance.as_str() {
                "light" => true,
                "dark" => false,
                _ => return Err("Theme variants may only be named light or dark.".into()),
            };
            if variant_light == light {
                return Err("Theme variants must not repeat the base appearance.".into());
            }
            variants.insert(
                appearance.clone(),
                Value::Object(colors(overrides, variant_light)?),
            );
        }
    }
    object.insert("version".into(), Value::from(1));
    object.insert("id".into(), Value::String(id));
    object.insert("name".into(), Value::String(name.clone()));
    object.insert("colors".into(), Value::Object(base.clone()));
    if !variants.is_empty() {
        object.insert("variants".into(), Value::Object(variants.clone()));
    } else {
        object.remove("variants");
    }
    if let Some(collection) = object.get_mut("collection").and_then(Value::as_object_mut) {
        let id = collection["id"].clone();
        let label = collection["label"].as_str().unwrap().trim().to_string();
        *collection = serde_json::json!({"id":id,"label":label})
            .as_object()
            .unwrap()
            .clone();
    }
    if object.get("managed") != Some(&Value::Bool(true)) {
        object.remove("managed");
    }
    if object.get("css").and_then(Value::as_str) == Some("") {
        object.remove("css");
    }
    object.retain(|key, _| {
        [
            "version",
            "id",
            "name",
            "appearance",
            "colors",
            "variants",
            "collection",
            "managed",
            "css",
        ]
        .contains(&key.as_str())
    });
    let (light, selected) = match variant {
        None => (light, &base),
        Some(appearance) => (
            appearance == "light",
            variants
                .get(appearance)
                .and_then(Value::as_object)
                .ok_or("This theme has no such appearance variant.")?,
        ),
    };
    let parsed = selected
        .iter()
        .map(|(role, value)| {
            (
                role.clone(),
                css(value.as_str().expect("validated color string")).expect("validated CSS color"),
            )
        })
        .collect();
    let aurora = file.get("css").and_then(Value::as_str).is_some_and(is_aurora_css);
    Ok(Imported {
        aurora,
        file,
        light,
        label: name,
        colors: parsed,
        ansi,
    })
}

impl Imported {
    pub fn role(&self, role: &str) -> CssColor {
        self.colors[role]
    }
    pub fn has_css(&self) -> bool {
        self.file
            .get("css")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    }
    pub fn variants(&self) -> impl Iterator<Item = &str> {
        self.file
            .get("variants")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|v| v.keys().map(String::as_str))
    }
    /// Terminal UI has no alpha channel: composite against the corresponding surface, never discard alpha.
    pub fn opaque(&self, role: &str, surface: &str) -> Color {
        let over = |c: CssColor, bg: [f32; 3]| {
            let channels = [c.r, c.g, c.b];
            std::array::from_fn::<_, 3, _>(|i| channels[i] * c.a + bg[i] * (1.0 - c.a))
        };
        let canvas = over(self.role("canvas"), [if self.light { 1.0 } else { 0.0 }; 3]);
        let bg = if surface == "canvas" {
            canvas
        } else {
            over(self.role(surface), canvas)
        };
        let rgb = if role == "canvas" {
            canvas
        } else if role == surface {
            bg
        } else {
            over(self.role(role), bg)
        };
        let [r, g, b] = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
        Color::Rgb(r, g, b)
    }
    pub fn theme(self, name: &str) -> Theme {
        let mut t = builtin(if self.light { "paper" } else { "github-dark" });
        t.name = name.into();
        t.bg = self.opaque("terminalBackground", "canvas");
        t.fg = self.opaque("terminalForeground", "terminalBackground");
        t.accent = self.opaque("accent", "canvas");
        t.shine = self.opaque("focus", "canvas");
        t.frame = self.opaque("border", "canvas");
        t.muted = self.opaque("textMuted", "canvas");
        t.user = self.opaque("accentSurface", "canvas");
        t.inline = self.opaque("warningForeground", "canvas");
        t.danger = self.opaque("errorForeground", "canvas");
        t.good = self.opaque("updateForeground", "canvas");
        t.panel = Some(self.opaque("sidebar", "canvas"));
        t.cursor = Some(self.opaque("terminalCursor", "terminalBackground"));
        t.ansi = self.ansi.or(t.ansi);
        t.imported = Some(Arc::new(self));
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MINIMAL: &str =
        r##"{"version":1,"name":"A test","appearance":"dark","colors":{"accent":"#ff00aa"}}"##;

    #[test]
    fn real_published_palettes_import_and_round_trip() {
        for (name, source, canvas, light) in [
            (
                "Catppuccin Mocha",
                include_str!("../../tests/fixtures/themes/catppuccin/mocha.json"),
                "#1e1e2e",
                false,
            ),
            (
                "Dracula",
                include_str!("../../tests/fixtures/themes/dracula/dracula.json"),
                "#282a36",
                false,
            ),
            (
                "Nord",
                include_str!("../../tests/fixtures/themes/nord/nord-color-theme.json"),
                "#2e3440",
                false,
            ),
            (
                "One Dark Pro",
                include_str!("../../tests/fixtures/themes/one-dark-pro/OneDark-Pro.json"),
                "#282c34",
                false,
            ),
            (
                "Vitesse Dark",
                include_str!("../../tests/fixtures/themes/vitesse/vitesse-dark.json"),
                "#121212",
                false,
            ),
            (
                "Solarized Light",
                include_str!("../../tests/fixtures/themes/vscode/solarized-light-color-theme.json"),
                "#fdf6e3",
                true,
            ),
        ] {
            let theme = parse(source, name, None).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(theme.colors.len(), ROLES.len(), "{name}");
            let references: Value = serde_json::from_str(include_str!(
                "../../tests/fixtures/themes/t3code/vscode-expected.json"
            ))
            .unwrap();
            let key = match name {
                "Catppuccin Mocha" => "catppuccin/mocha.json",
                "Dracula" => "dracula/dracula.json",
                "Nord" => "nord/nord-color-theme.json",
                "One Dark Pro" => "one-dark-pro/OneDark-Pro.json",
                "Vitesse Dark" => "vitesse/vitesse-dark.json",
                _ => "vscode/solarized-light-color-theme.json",
            };
            for role in ROLES {
                let expected = css(references["themes"][key]["colors"][role].as_str().unwrap())
                    .unwrap()
                    .to_rgba8();
                assert_eq!(
                    theme.role(role).to_rgba8(),
                    expected,
                    "{name}: {role} agrees with pinned T3 importer"
                );
            }
            assert_eq!(theme.light, light, "{name}");
            assert_eq!(
                theme.role("canvas").to_rgba8(),
                css(canvas).unwrap().to_rgba8(),
                "{name} canvas"
            );
            assert!(
                theme.ansi.is_some(),
                "{name} includes a complete terminal palette"
            );
            let original = jsonc(source).unwrap();
            if let Some(black) = original["colors"]["terminal.ansiBlack"].as_str() {
                let [r, g, b, _] = css(black).unwrap().to_rgba8();
                assert_eq!(
                    theme.ansi.unwrap()[0],
                    Color::Rgb(r, g, b),
                    "{name} ANSI black"
                );
            }
            let exported = parse(&theme.file.to_string(), name, None).unwrap();
            for role in ROLES {
                assert_eq!(
                    theme.role(role).to_rgba8(),
                    exported.role(role).to_rgba8(),
                    "{name}: {role} survives native export"
                );
            }
        }
    }

    #[test]
    fn vscode_alpha_and_wide_gamut_match_the_reference() {
        let inputs: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/themes/t3code/vscode-edgecases.json"
        ))
        .unwrap();
        let references: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/themes/t3code/vscode-expected.json"
        ))
        .unwrap();
        for (name, input) in inputs.as_object().unwrap() {
            let theme = parse(&input.to_string(), name, None).unwrap();
            assert_eq!(
                theme.light,
                references["edgecases"][name]["appearance"] == "light"
            );
            for role in ROLES {
                let expected = css(references["edgecases"][name]["colors"][role]
                    .as_str()
                    .unwrap())
                .unwrap()
                .to_rgba8();
                assert_eq!(theme.role(role).to_rgba8(), expected, "{name}: {role}");
            }
        }
    }

    #[test]
    fn real_native_exports_keep_modes_and_color_families() {
        let file = include_str!("../../tests/fixtures/themes/t3code/t3-chat-export.json");
        let base = parse(file, "t3", None).unwrap();
        let dark = parse(file, "t3", Some("dark")).unwrap();
        assert!(base.light && !dark.light);
        assert_eq!(base.colors.len(), 61);
        assert_ne!(base.role("canvas"), dark.role("canvas"));
        let literals = parse(
            include_str!("../../tests/fixtures/themes/t3code/literal-colors.json"),
            "literals",
            None,
        )
        .unwrap();
        assert_eq!(literals.colors.len(), 61);
    }

    #[test]
    fn native_defaults_and_variants_are_independent() {
        let t = parse(MINIMAL, "test", None).unwrap();
        assert_eq!(t.colors.len(), 61);
        assert_eq!(t.file["id"], "a-test");
        let mut file: Value = serde_json::from_str(MINIMAL).unwrap();
        file["variants"] = serde_json::json!({"light":{"canvas":"white"}});
        let text = file.to_string();
        let v = parse(&text, "test", Some("light")).unwrap();
        assert!(v.light);
        assert_ne!(
            v.role("accent"),
            t.role("accent"),
            "variant omissions use their own defaults"
        );
        assert_eq!(v.role("canvas").to_rgba8(), [255; 4]);
        assert!(
            v.file["variants"]["light"]["canvas"]
                .as_str()
                .unwrap()
                .starts_with("oklch(")
        );
        let round = parse(&v.file.to_string(), "test", Some("light")).unwrap();
        for role in ROLES {
            assert_eq!(
                round.role(role).to_rgba8(),
                v.role(role).to_rgba8(),
                "{role} survives export"
            );
        }
    }

    #[test]
    fn validation_matches_native_contract() {
        let base: Value = serde_json::from_str(MINIMAL).unwrap();
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("id", serde_json::json!("t3-chat")),
            ("id", serde_json::json!("../escape")),
            ("colors", serde_json::json!({})),
            ("colors", serde_json::json!({"unknown":"red"})),
            ("colors", serde_json::json!({"accent":"var(--red)"})),
            ("colors", serde_json::json!({"accent":"currentColor"})),
            ("variants", serde_json::json!({"dark":{"canvas":"red"}})),
            ("variants", serde_json::json!({"light":{}})),
            ("css", Value::Bool(false)),
            (
                "collection",
                serde_json::json!({"id":"../bad","label":"Test"}),
            ),
        ] {
            let mut file = base.clone();
            file[key] = value;
            assert!(
                parse(&file.to_string(), "test", None).is_err(),
                "{key}: {file}"
            );
        }
        assert!(parse(&format!("// comment\n{MINIMAL}"), "test", None).is_err());
        for id in ["1", "a", "a-", "0-dark"] {
            let mut v = base.clone();
            v["id"] = id.into();
            assert!(parse(&v.to_string(), "test", None).is_ok());
        }
    }

    #[test]
    fn jsonc_is_not_json5_and_native_precedence_is_explicit() {
        for source in [
            "{colors:{\"editor.background\":\"#000\"}}",
            "{'colors':{'editor.background':'#000'}}",
            "{\"colors\":{\"editor.background\":\"#000\"},\"extra\":0x12}",
        ] {
            assert!(parse(source, "bad", None).is_err(), "{source}");
        }
        let native = MINIMAL.replace("\"version\":1", "\"version\":1.0");
        assert!(
            parse(&native, "numeric", None).is_ok(),
            "JavaScript treats 1.0 as version 1"
        );
        let mut mixed: Value = serde_json::from_str(MINIMAL).unwrap();
        mixed["colors"]["editor.background"] = "#000000".into();
        assert!(
            parse(&mixed.to_string(), "mixed", None)
                .unwrap_err()
                .contains("not a supported"),
            "native version wins over dotted workbench keys"
        );
        let mut native: Value = serde_json::from_str(MINIMAL).unwrap();
        native["sidebarArtwork"] = true.into();
        native["managed"] = false.into();
        let result = parse(&native.to_string(), "ignored", None).unwrap();
        assert!(
            result.file.get("sidebarArtwork").is_none() && result.file.get("managed").is_none()
        );
        assert_ne!(id_from_name("日本語"), id_from_name("中文"));
    }

    #[test]
    fn css_formats_and_alpha_survive() {
        for value in [
            "#abcd",
            "#12345678",
            "rebeccapurple",
            "transparent",
            "rgb(20 30 40 / 50%)",
            "hsl(210 50% 50%)",
            "hwb(210 10% 20%)",
            "lab(50 20 30)",
            "lch(50 20 270)",
            "oklab(.5 .1 .1)",
            "oklch(60% 0.2 280 / .3)",
            "color(display-p3 1 .5 0)",
        ] {
            assert!(css(value).is_some(), "{value}");
        }
        assert_eq!(css("oklch(.5 .1 50 / none)").unwrap().a, 0.0);
        let mut f: Value = serde_json::from_str(MINIMAL).unwrap();
        f["css"] = ".some-class { border-radius: 8px; }".into();
        f["collection"] = serde_json::json!({"id":"openvsx:example.theme","label":"Example"});
        f["managed"] = true.into();
        f["colors"]["terminalSelection"] = "#ff000080".into();
        let t = parse(&f.to_string(), "test", None).unwrap();
        assert!(t.has_css());
        assert_eq!(t.role("terminalSelection").to_rgba8(), [255, 0, 0, 128]);
        assert!(
            t.file["colors"]["terminalSelection"]
                .as_str()
                .unwrap()
                .starts_with("oklch(")
        );
        assert_eq!(t.file["collection"], f["collection"]);
        assert_eq!(t.file["managed"], true);
    }
}
