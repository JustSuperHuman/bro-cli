//! MCP Apps: what showing an app's page takes that the desktop app and the phone share. bro's host page (a
//! sandboxed frame for the server's page, and the host side of the protocol), what it loads the page with, where
//! read pages are kept, the HTML in a read, and what a page may ask its server. The desktop app lays the host page
//! over a card in its own web view (bro-tui's `app/gui/apps.rs`); the phone loads it from the bridge
//! (`/api/sessions/{id}/apps/{call}`), page and all.

use crate::sessions::conversation::McpApp;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A colour as sRGB channels, 0 to 1.
pub type Rgb = [f32; 3];

/// Where a page is shown, and in what: what its host context says.
#[derive(Debug, Clone)]
pub struct Frame<'a> {
    /// the call's id in the transcript
    pub call: &'a str,
    pub light: bool,
    /// the card's fill, which the page sits on
    pub fill: Rgb,
    /// the room it has (CSS px) and the tallest it may grow
    pub width: f32,
    pub max_h: f32,
    /// a phone: touch, no hover
    pub mobile: bool,
    /// bro's version
    pub version: &'a str,
}

/// What a page asked of its server, as a job for the session's mod (`None`: not something a page may ask).
pub fn job(app: &McpApp, method: &str, params: &Value) -> Option<Value> {
    match method {
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str)?;
            Some(json!({ "kind": "call", "server": app.server, "tool": name, "arguments": params.get("arguments").cloned().unwrap_or_else(|| json!({})) }))
        }
        "resources/read" => {
            let uri = params.get("uri").and_then(Value::as_str)?;
            Some(json!({ "kind": "read", "server": app.server, "uri": uri }))
        }
        _ => None,
    }
}

/// JSON to put in a script: nothing in it can end the script or a line.
pub fn script_json(v: &Value) -> String {
    v.to_string().replace('<', "\\u003c").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// A colour as CSS.
fn css(c: Rgb) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// `top` at `alpha` over `under`.
fn over(top: Rgb, alpha: f32, under: Rgb) -> Rgb {
    [0, 1, 2].map(|i| top[i] * alpha + under[i] * (1.0 - alpha))
}

/// MCP Apps' style variables in bro's theme, for pages that take them: the card's fill, the theme's text and
/// borders, the system's fonts, bro's corners.
pub fn styles(fill: Rgb, light: bool) -> Value {
    let ink: Rgb = if light { [0x1b as f32 / 255.0, 0x1b as f32 / 255.0, 0x1f as f32 / 255.0] } else { [0xf2 as f32 / 255.0, 0xf2 as f32 / 255.0, 0xf5 as f32 / 255.0] };
    let vars = json!({
        "--color-background-primary": css(fill),
        "--color-background-secondary": css(over(ink, 0.04, fill)),
        "--color-background-tertiary": css(over(ink, 0.08, fill)),
        "--color-background-inverse": css(ink),
        "--color-text-primary": css(ink),
        "--color-text-secondary": css(over(ink, 0.68, fill)),
        "--color-text-tertiary": css(over(ink, 0.48, fill)),
        "--color-text-inverse": css(fill),
        "--color-border-primary": css(over(ink, 0.14, fill)),
        "--color-border-secondary": css(over(ink, 0.08, fill)),
        "--font-sans": "\"Segoe UI Variable Text\", \"Segoe UI\", -apple-system, BlinkMacSystemFont, Inter, system-ui, sans-serif",
        "--font-mono": "\"Cascadia Code\", \"SF Mono\", Consolas, ui-monospace, monospace",
        "--border-radius-sm": "4px",
        "--border-radius-md": "8px",
        "--border-radius-lg": "12px",
    });
    json!({ "variables": vars })
}

/// The parts of a page's host context that change while it shows (the theme, its room): told to the page when
/// they do.
pub fn context(f: &Frame) -> Value {
    json!({ "theme": if f.light { "light" } else { "dark" }, "styles": styles(f.fill, f.light), "containerDimensions": { "width": f.width.round(), "maxHeight": f.max_h.round() } })
}

/// What the host page loads a page with (`window.__broApp.load(<this>)`): the page, the call's input and result,
/// and the host context, as a script's argument.
pub fn payload(app: &McpApp, html: &str, f: &Frame) -> String {
    let mut context = context(f);
    if let Some(c) = context.as_object_mut() {
        c.insert("displayMode".into(), json!("inline"));
        c.insert("availableDisplayModes".into(), json!(["inline"]));
        c.insert("platform".into(), json!(if f.mobile { "mobile" } else { "desktop" }));
        c.insert("userAgent".into(), json!(format!("bro/{}", f.version)));
        c.insert("deviceCapabilities".into(), json!({ "touch": f.mobile, "hover": !f.mobile }));
        c.insert("safeAreaInsets".into(), json!({ "top": 0, "right": 0, "bottom": 0, "left": 0 }));
        c.insert("toolInfo".into(), json!({ "id": f.call, "tool": { "name": app.tool } }));
    }
    script_json(&json!({ "html": html, "input": app.input, "result": app.result, "version": f.version, "context": context }))
}

/// The host page with its page loading in it: what the phone's web view is given.
pub fn host_page(payload: &str) -> String {
    let at = HOST.rfind("</body>").unwrap_or(HOST.len());
    format!("{}<script>window.__broApp.load({payload});</script>{}", &HOST[..at], &HOST[at..])
}

/// Where read pages are kept under `root` (`~/.bro`): `mcp-apps/<server>/<resource>.html`, each name one plain
/// file name.
pub fn page_path(root: &Path, server: &str, uri: &str) -> PathBuf {
    let safe = |s: &str| -> String {
        let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
        let s = if s.len() > 150 { s[s.len() - 150..].to_string() } else { s };
        if s.is_empty() || s.starts_with('.') { format!("_{s}") } else { s }
    };
    let name = safe(uri.trim_start_matches("ui://").trim_end_matches(".html"));
    root.join("mcp-apps").join(safe(server)).join(format!("{name}.html"))
}

/// The HTML in what the mod read (`resources/read`'s `{contents: [{text, mimeType}]}`).
pub fn page_text(v: &Value) -> Result<String, String> {
    if let Some(e) = v.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        return Err(e.to_string());
    }
    let contents = v.get("contents").and_then(Value::as_array).cloned().unwrap_or_default();
    contents
        .iter()
        .filter(|c| c.get("mimeType").and_then(Value::as_str).is_none_or(|m| m.starts_with("text/html")))
        .find_map(|c| c.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).map(str::to_string))
        .ok_or_else(|| "the server sent no page".to_string())
}

// ------------------------------------------------------------------------------------------------- the host page

/// bro's host page: a sandboxed frame for the server's page, and the host side of MCP Apps (`ui/initialize`, the
/// call's input and result, size changes, links, messages, its server's tools). It talks to bro with
/// `window.ipc.postMessage(json)` (the phone: `window.ReactNativeWebView.postMessage`): `host` (ready for the page), `size`, `wheel`, `esc`, `open`, `message`, `ask`.
pub const HOST: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="color-scheme" content="light dark">
<style>
html, body { margin: 0; padding: 0; height: 100%; overflow: hidden; background: transparent; }
iframe { display: block; border: 0; width: 100%; height: 100%; background: transparent; }
</style></head>
<body><iframe id="app" sandbox="allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads" allow="clipboard-write; fullscreen; autoplay" referrerpolicy="no-referrer"></iframe>
<script>
(function () {
  // bro's web view (wry's `window.ipc`), or the phone's (react-native-webview)
  var ipc = function (m) { try { (window.ipc || window.ReactNativeWebView).postMessage(JSON.stringify(m)); } catch (e) {} };
  var frame = document.getElementById('app');
  var P = null, initialized = false, waiting = {}, n = 0;
  var send = function (m) { if (frame.contentWindow) frame.contentWindow.postMessage(m, '*'); };
  var respond = function (id, result) { send({ jsonrpc: '2.0', id: id, result: result }); };
  var fail = function (id, code, message) { send({ jsonrpc: '2.0', id: id, error: { code: code, message: String(message) } }); };
  var notify = function (method, params) { send({ jsonrpc: '2.0', method: method, params: params }); };
  var json = function (v) { return JSON.stringify(v === undefined ? null : v).replace(/</g, '\\u003c').replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029'); };
  var context = function () {
    var c = Object.assign({}, P.context);
    c.locale = navigator.language;
    try { c.timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone; } catch (e) {}
    return c;
  };
  // in the page, before its own scripts: the wheel and Esc for bro, and the Apps SDK's window.openai
  var prelude = function () {
    var r = P.result || {};
    var g = { toolInput: P.input || {}, toolOutput: r.structuredContent === undefined ? null : r.structuredContent, toolResponseMetadata: r._meta || null, theme: P.context.theme, locale: navigator.language, maxHeight: (P.context.containerDimensions || {}).maxHeight };
    return '<script>(' + PRELUDE.toString() + ')(' + json(g) + ');<\/script>';
  };
  function PRELUDE(g) {
    var post = function (m) { try { parent.postMessage(m, '*'); } catch (e) {} };
    var scrolls = function (n, dy) {
      var s = getComputedStyle(n), o = s.overflowY;
      if (n !== document.scrollingElement && o !== 'auto' && o !== 'scroll' && o !== 'overlay') return false;
      if (n.scrollHeight <= n.clientHeight + 1) return false;
      return dy < 0 ? n.scrollTop > 0 : n.scrollTop + n.clientHeight < n.scrollHeight - 1;
    };
    addEventListener('wheel', function (e) {
      if (e.ctrlKey) return;
      var dy = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? innerHeight : 1);
      if (!dy) return;
      for (var n = e.target; n && n.nodeType === 1; n = n.parentElement) if (scrolls(n, dy)) return;
      if (document.scrollingElement && scrolls(document.scrollingElement, dy)) return;
      post({ __bro: 'wheel', dy: dy });
    }, { passive: true, capture: true });
    addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && !e.defaultPrevented && !e.ctrlKey && !e.altKey && !e.metaKey) post({ __bro: 'esc' });
    });
    var waiting = {}, n = 0;
    var rpc = function (method, params) {
      return new Promise(function (ok, no) { var id = 'bro-' + (++n); waiting[id] = [ok, no]; post({ jsonrpc: '2.0', id: id, method: method, params: params }); });
    };
    addEventListener('message', function (e) {
      var m = e.data;
      if (e.source !== parent || !m || typeof m.id !== 'string' || !waiting[m.id]) return;
      var w = waiting[m.id]; delete waiting[m.id];
      if (m.error) w[1](m.error); else w[0](m.result);
    });
    var api = {
      displayMode: 'inline', view: { mode: 'inline' }, widgetState: null,
      safeArea: { insets: { top: 0, right: 0, bottom: 0, left: 0 } },
      userAgent: { device: { type: 'desktop' }, capabilities: { hover: true, touch: false } },
      setWidgetState: function (s) { window.openai.widgetState = s; return Promise.resolve(); },
      callTool: function (name, args) { return rpc('tools/call', { name: name, arguments: args || {} }); },
      sendFollowUpMessage: function (o) { return rpc('ui/message', { role: 'user', content: [{ type: 'text', text: (o && o.prompt) || '' }] }); },
      openExternal: function (o) { return rpc('ui/open-link', { url: (o && o.href) || '' }); },
      requestDisplayMode: function () { return Promise.resolve({ mode: 'inline' }); },
      requestClose: function () { return Promise.resolve(); },
      notifyIntrinsicHeight: function (o) { if (o && o.height) post({ jsonrpc: '2.0', method: 'ui/notifications/size-changed', params: { height: o.height } }); }
    };
    window.openai = Object.assign(g, api);
    addEventListener('DOMContentLoaded', function () {
      try { dispatchEvent(new CustomEvent('openai:set_globals', { detail: { globals: window.openai } })); } catch (e) {}
    });
  }
  var compose = function (html) {
    var p = prelude(), at = /<head[^>]*>/i.exec(html) || /<html[^>]*>/i.exec(html);
    if (!at) return p + html;
    var cut = at.index + at[0].length;
    return html.slice(0, cut) + p + html.slice(cut);
  };
  window.__broApp = {
    load: function (p) { P = p; initialized = false; frame.srcdoc = compose(p.html); },
    // where the whole card is, in physical px, in a view that shows only the part on screen (macOS: View::place)
    clip: function (x, y, w, h) {
      var r = window.devicePixelRatio || 1, s = frame.style;
      s.position = 'absolute'; s.left = x / r + 'px'; s.top = y / r + 'px'; s.width = w / r + 'px'; s.height = h / r + 'px';
    },
    context: function (c) {
      if (!P) return;
      Object.assign(P.context, c);
      if (initialized) notify('ui/notifications/host-context-changed', c);
    },
    answer: function (k, ok, v) {
      var id = waiting[k];
      if (id === undefined) return;
      delete waiting[k];
      if (ok) respond(id, v); else fail(id, -32603, v);
    }
  };
  window.addEventListener('message', function (e) {
    if (!P || e.source !== frame.contentWindow) return;
    var m = e.data;
    if (m && m.__bro) { ipc({ t: m.__bro, dy: m.dy }); return; }
    if (!m || m.jsonrpc !== '2.0' || typeof m.method !== 'string') return;
    var has = m.id !== undefined && m.id !== null, p = m.params || {};
    switch (m.method) {
      case 'ui/initialize':
        return respond(m.id, {
          protocolVersion: p.protocolVersion || '2026-01-26',
          hostInfo: { name: 'bro', version: P.version },
          hostCapabilities: { openLinks: {}, serverTools: {}, serverResources: {}, logging: {}, message: { text: {} } },
          hostContext: context()
        });
      case 'ui/notifications/initialized':
        if (!initialized) {
          initialized = true;
          // the call has finished by the time its page opens: a result with structured content says it all, and
          // pages that show a progress screen take an input on its own as a call still running (JustGains' card
          // then drops the earlier result as stale). A result without it goes after its input, as the spec has it.
          var r = P.result || {};
          if (r.structuredContent === undefined || r.structuredContent === null) notify('ui/notifications/tool-input', { arguments: P.input || {} });
          notify('ui/notifications/tool-result', r);
        }
        return;
      case 'ui/notifications/size-changed': ipc({ t: 'size', h: p.height, w: p.width }); return;
      case 'ping': if (has) respond(m.id, {}); return;
      case 'tools/call':
      case 'resources/read':
        if (!has) return;
        var k = 'r' + (++n); waiting[k] = m.id;
        ipc({ t: 'ask', n: k, method: m.method, params: p });
        return;
      case 'ui/open-link': ipc({ t: 'open', url: p.url }); if (has) respond(m.id, {}); return;
      case 'ui/message': ipc({ t: 'message', params: p }); if (has) respond(m.id, {}); return;
      case 'ui/request-display-mode': if (has) respond(m.id, { mode: 'inline' }); return;
      case 'ui/update-model-context': if (has) respond(m.id, {}); return;
      case 'notifications/message': return;
      default: if (has) fail(m.id, -32601, 'bro does not support ' + m.method);
    }
  });
  ipc({ t: 'host' });
})();
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> McpApp {
        McpApp { server: "claude_ai_JustGains".into(), tool: "show_workout".into(), resource: "ui://widget/workout-v84.html".into(), input: json!({}), result: json!({}) }
    }

    #[test]
    fn a_page_asks_its_own_server_and_nothing_else() {
        let a = app();
        assert_eq!(job(&a, "tools/call", &json!({"name": "get_workout_program", "arguments": {"id": 1}})), Some(json!({"kind": "call", "server": "claude_ai_JustGains", "tool": "get_workout_program", "arguments": {"id": 1}})));
        assert_eq!(job(&a, "tools/call", &json!({"name": "x"}))
            .and_then(|j| j.get("arguments").cloned()), Some(json!({})), "no arguments are empty ones");
        assert_eq!(job(&a, "resources/read", &json!({"uri": "ui://widget/other.html"})), Some(json!({"kind": "read", "server": "claude_ai_JustGains", "uri": "ui://widget/other.html"})));
        assert_eq!(job(&a, "tools/call", &json!({})), None, "a call names its tool");
        assert_eq!(job(&a, "sampling/createMessage", &json!({})), None);
    }

    #[test]
    fn a_read_page_is_its_html() {
        let ok = json!({"contents": [{"uri": "ui://w", "mimeType": "text/html;profile=mcp-app", "text": "<html></html>"}]});
        assert_eq!(page_text(&ok).as_deref(), Ok("<html></html>"));
        assert!(page_text(&json!({"contents": [], "error": "Server not found"})).unwrap_err().contains("not found"));
        assert!(page_text(&json!({"contents": [{"mimeType": "image/png", "blobSavedTo": "x"}]})).is_err(), "only a page");
    }

    #[test]
    fn script_json_cant_end_the_script() {
        let s = script_json(&json!({"html": "</script><script>alert(1)</script>\u{2028}"}));
        assert!(!s.contains("</script>") && !s.contains('\u{2028}'), "{s}");
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["html"], "</script><script>alert(1)</script>\u{2028}");
    }

    #[test]
    fn pages_are_kept_by_server_and_resource() {
        let root = std::path::Path::new("root");
        let p = page_path(root, "claude_ai_JustGains", "ui://widget/workout-v84.html");
        assert_eq!(p, root.join("mcp-apps").join("claude_ai_JustGains").join("widget_workout-v84.html"));
        // nothing a server names leaves the folder
        let odd = page_path(root, "..", "ui://../../x?y");
        assert_eq!(odd.parent().and_then(|d| d.parent()), Some(root.join("mcp-apps").as_path()), "{}", odd.display());
        assert!(odd.components().all(|c| matches!(c, std::path::Component::Normal(_))), "{}", odd.display());
    }

    #[test]
    fn colours_are_css_and_mix_over_the_fill() {
        assert_eq!(css([0x12 as f32 / 255.0, 0xab as f32 / 255.0, 1.0]), "#12abff");
        assert_eq!(css(over([1.0, 1.0, 1.0], 0.5, [0.0, 0.0, 0.0])), "#808080");
    }

    #[test]
    fn the_phones_host_page_loads_its_page() {
        let a = app();
        let f = Frame { call: "t1", light: false, fill: [0.1, 0.1, 0.1], width: 360.0, max_h: 3000.0, mobile: true, version: "2.1.17" };
        let page = host_page(&payload(&a, "<html><body>hi</body></html>", &f));
        assert!(page.ends_with("</body></html>"), "the page loads at the end of the host's body");
        let load = page.rfind("window.__broApp.load(").expect("loads");
        assert!(load > page.find("window.__broApp = {").unwrap(), "after the host's own script");
        assert!(page.contains("\"platform\":\"mobile\"") && page.contains("\"touch\":true"), "a phone's context");
        assert!(page.contains("window.ReactNativeWebView"), "it talks to the phone's web view");
    }
}
