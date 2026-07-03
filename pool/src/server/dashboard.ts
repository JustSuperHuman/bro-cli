/**
 * Self-contained status dashboard. Served at `/`, it polls `/api/status` and
 * renders one card per account: auth state, plan/tier, token expiry, rolling
 * usage, and rate-limit cooldown. No external assets (CSP-friendly).
 *
 * Visual language mirrors Claude.ai: warm paper background, editorial serif
 * display type, clay accent, hairline borders, restrained shadows. Supports a
 * light/dark theme toggle (dark is Claude's warm charcoal).
 */

export function dashboardHtml(): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Claude Max Pool</title>
<style>
  :root {
    --bg: #f5f4ef; --surface: #ffffff; --surface-2: #faf9f5;
    --border: #e7e4da; --border-strong: #dbd7ca;
    --text: #1c1b19; --muted: #6e6c64; --faint: #93908544;
    --accent: #c96442; --accent-hover: #b15537; --accent-soft: #f2e7e0;
    --ready: #5b9a6e; --warn: #b9812f; --err: #c0553f;
    --track: #eceae1;
    --shadow: 0 1px 2px rgba(28,27,25,.04), 0 2px 6px rgba(28,27,25,.04);
    --shadow-hover: 0 2px 4px rgba(28,27,25,.06), 0 8px 24px rgba(28,27,25,.06);
    --serif: "Iowan Old Style", "Palatino Linotype", Palatino, Georgia, "Times New Roman", serif;
    --sans: ui-sans-serif, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
    --mono: ui-monospace, "SF Mono", "Cascadia Code", Menlo, Consolas, monospace;
  }
  /* Dark palette applied by system preference (unless overridden to light) OR explicit toggle */
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) {
      --bg: #262624; --surface: #302f2c; --surface-2: #35342f;
      --border: #3e3d38; --border-strong: #4a483f;
      --text: #f4f2ec; --muted: #a6a399; --faint: #ffffff10;
      --accent: #d97757; --accent-hover: #e28a6d; --accent-soft: #3a2f28;
      --ready: #6fb283; --warn: #d2a24a; --err: #db7b63;
      --track: #3a3934;
      --shadow: 0 1px 2px rgba(0,0,0,.24), 0 2px 6px rgba(0,0,0,.20);
      --shadow-hover: 0 2px 4px rgba(0,0,0,.3), 0 10px 28px rgba(0,0,0,.32);
      color-scheme: dark;
    }
  }
  :root[data-theme="dark"] {
    --bg: #262624; --surface: #302f2c; --surface-2: #35342f;
    --border: #3e3d38; --border-strong: #4a483f;
    --text: #f4f2ec; --muted: #a6a399; --faint: #ffffff10;
    --accent: #d97757; --accent-hover: #e28a6d; --accent-soft: #3a2f28;
    --ready: #6fb283; --warn: #d2a24a; --err: #db7b63;
    --track: #3a3934;
    --shadow: 0 1px 2px rgba(0,0,0,.24), 0 2px 6px rgba(0,0,0,.20);
    --shadow-hover: 0 2px 4px rgba(0,0,0,.3), 0 10px 28px rgba(0,0,0,.32);
    color-scheme: dark;
  }

  * { box-sizing: border-box; }
  html { -webkit-font-smoothing: antialiased; text-rendering: optimizeLegibility; }
  body { margin: 0; background: var(--bg); color: var(--text); font: 15px/1.6 var(--sans);
    letter-spacing: -0.005em; }

  header { border-bottom: 1px solid var(--border);
    display: flex; align-items: center; gap: 14px; padding: 18px 28px;
    position: sticky; top: 0; background: color-mix(in srgb, var(--bg) 88%, transparent);
    backdrop-filter: saturate(1.2) blur(8px); z-index: 10; }
  .brand { display: flex; align-items: center; gap: 12px; }
  .logo { width: 34px; height: 34px; border-radius: 9px; background: var(--accent);
    display: grid; place-items: center; color: #fff; font-family: var(--serif); font-size: 20px;
    font-weight: 600; line-height: 1; }
  h1 { font-family: var(--serif); font-size: 21px; margin: 0; font-weight: 500; letter-spacing: -0.01em; }
  .sub { color: var(--muted); font-size: 13px; margin-top: 1px; }
  .spacer { flex: 1; }
  .chips { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .chip { font-size: 12.5px; padding: 5px 11px; border-radius: 999px;
    border: 1px solid var(--border); background: var(--surface); color: var(--muted); white-space: nowrap; }
  .chip b { color: var(--text); font-weight: 600; }
  .chip.time { border: none; background: transparent; padding-left: 2px; }
  .icon-btn { width: 34px; height: 34px; border-radius: 9px; border: 1px solid var(--border);
    background: var(--surface); color: var(--muted); cursor: pointer; display: grid; place-items: center;
    font-size: 15px; transition: color .15s, border-color .15s; }
  .icon-btn:hover { color: var(--text); border-color: var(--border-strong); }

  main { padding: 32px 28px 8px; max-width: 1120px; margin: 0 auto; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 18px; }

  .card { background: var(--surface); border: 1px solid var(--border); border-radius: 16px;
    padding: 20px 22px; box-shadow: var(--shadow); transition: box-shadow .18s, transform .18s, border-color .18s; }
  .card:hover { box-shadow: var(--shadow-hover); transform: translateY(-1px); }
  .card.down { background: var(--surface-2); }
  .card-top { display: flex; align-items: center; gap: 10px; margin-bottom: 16px; }
  .status { display: inline-flex; align-items: center; gap: 8px; }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex: none; }
  .dot.ok { background: var(--ready); } .dot.warn { background: var(--warn); } .dot.err { background: var(--err); }
  .acct-name { font-family: var(--serif); font-weight: 500; font-size: 17px; letter-spacing: -0.01em; }
  .state { font-size: 11px; text-transform: uppercase; letter-spacing: .07em; color: var(--muted); }
  .chip.plan { margin-left: auto; font-size: 11.5px; padding: 3px 9px;
    background: var(--accent-soft); border-color: transparent; color: var(--accent); font-weight: 500; }

  .dl { display: grid; grid-template-columns: auto 1fr; gap: 7px 16px; font-size: 13.5px; }
  .dl .k { color: var(--muted); }
  .dl .v { text-align: right; font-variant-numeric: tabular-nums; color: var(--text); }

  .bars { margin-top: 16px; display: grid; gap: 11px; }
  .bar-label { display: flex; justify-content: space-between; font-size: 12px; color: var(--muted); margin-bottom: 5px; }
  .bar-label .num { font-variant-numeric: tabular-nums; color: var(--text); }
  .bar { height: 5px; background: var(--track); border-radius: 999px; overflow: hidden; }
  .bar > span { display: block; height: 100%; background: var(--accent); border-radius: 999px; transition: width .4s ease; }

  .note { margin-top: 14px; padding-top: 13px; border-top: 1px solid var(--border);
    font-size: 12.5px; color: var(--warn); }
  .note.err { color: var(--err); }

  /* Attention banner */
  .banner { display: none; background: var(--surface); border: 1px solid var(--border);
    border-left: 3px solid var(--warn); border-radius: 12px; padding: 14px 18px; margin-bottom: 20px;
    font-size: 13.5px; color: var(--text); box-shadow: var(--shadow); }
  .banner b { font-weight: 600; }
  .banner code { font-family: var(--mono); background: var(--surface-2); border: 1px solid var(--border);
    padding: 2px 6px; border-radius: 5px; font-size: 12.5px; }

  /* Onboarding walkthrough */
  .onboard { max-width: 720px; margin: 8px auto 0; display: none; }
  .onboard-head { text-align: center; margin-bottom: 30px; }
  .onboard-head h2 { font-family: var(--serif); font-weight: 500; font-size: 27px; margin: 0 0 8px; letter-spacing: -0.015em; }
  .onboard-head p { margin: 0; color: var(--muted); font-size: 15px; }
  .steps { display: grid; gap: 14px; }
  .step { display: grid; grid-template-columns: 32px 1fr; gap: 16px; background: var(--surface);
    border: 1px solid var(--border); border-radius: 14px; padding: 20px 22px; box-shadow: var(--shadow); }
  .step .num { width: 30px; height: 30px; border-radius: 50%; border: 1.5px solid var(--accent);
    color: var(--accent); display: grid; place-items: center; font-family: var(--serif); font-weight: 600; font-size: 15px; }
  .step h3 { margin: 3px 0 8px; font-size: 15.5px; font-weight: 600; letter-spacing: -0.01em; }
  .step p { margin: 0 0 12px; color: var(--muted); font-size: 13.5px; }
  .step p:last-child { margin-bottom: 0; }
  .step code { font-family: var(--mono); background: var(--surface-2); border: 1px solid var(--border);
    padding: 1px 6px; border-radius: 5px; font-size: 12.5px; color: var(--text); }
  .cmd { display: flex; align-items: center; gap: 12px; background: var(--surface-2);
    border: 1px solid var(--border); border-radius: 9px; padding: 11px 14px; margin: 8px 0; }
  .cmd .prompt { color: var(--accent); font-family: var(--mono); user-select: none; }
  .cmd > code { flex: 1; background: none; border: none; padding: 0; font-size: 13px;
    white-space: pre-wrap; word-break: break-word; line-height: 1.5; }
  .cmd button { background: var(--surface); color: var(--muted); border: 1px solid var(--border);
    border-radius: 7px; padding: 5px 11px; font-size: 11.5px; font-family: var(--sans); cursor: pointer;
    transition: color .15s, border-color .15s; }
  .cmd button:hover { color: var(--text); border-color: var(--border-strong); }
  .step small { color: var(--muted); font-size: 12.5px; }
  .waiting { text-align: center; margin-top: 26px; color: var(--muted); font-size: 13px; }
  .waiting .live { display: inline-block; width: 7px; height: 7px; border-radius: 50%;
    background: var(--ready); margin-right: 7px; animation: pulse 1.6s ease-in-out infinite; vertical-align: middle; }
  @keyframes pulse { 0%,100% { opacity: .3; } 50% { opacity: 1; } }

  footer { color: var(--muted); font-size: 12.5px; text-align: center; padding: 30px 20px 36px; }
  footer code { font-family: var(--mono); background: var(--surface); border: 1px solid var(--border);
    padding: 2px 7px; border-radius: 5px; color: var(--muted); margin: 0 2px; }
</style>
</head>
<body>
<header>
  <div class="brand">
    <div class="logo">C</div>
    <div>
      <h1>Claude Max Pool</h1>
      <div class="sub">Many Claude plans, one endpoint</div>
    </div>
  </div>
  <div class="spacer"></div>
  <div class="chips">
    <span class="chip" id="p-accounts">accounts <b>&ndash;</b></span>
    <span class="chip" id="p-available">available <b>&ndash;</b></span>
    <span class="chip" id="p-window">window <b>&ndash;</b></span>
    <span class="chip time" id="p-updated">&ndash;</span>
    <button class="icon-btn" id="theme-toggle" title="Toggle theme" aria-label="Toggle theme">◐</button>
  </div>
</header>
<main>
  <div class="banner" id="banner"></div>
  <div class="grid" id="grid"></div>

  <section class="onboard" id="onboard">
    <div class="onboard-head">
      <h2>Let&rsquo;s set up your first account</h2>
      <p>Each account is a separate Claude Max or Team login the proxy can route between. Add as many as you like.</p>
    </div>
    <div class="steps">
      <div class="step">
        <div class="num">1</div>
        <div>
          <h3>Install the Claude CLI</h3>
          <p>The proxy uses Claude Code logins for account OAuth credentials. Serving requests uses the direct Anthropic backend by default.</p>
          <div class="cmd"><span class="prompt">$</span><code>npm install -g @anthropic-ai/claude-code</code><button data-copy>Copy</button></div>
          <p><small>Already installed? Skip ahead.</small></p>
        </div>
      </div>
      <div class="step">
        <div class="num">2</div>
        <div>
          <h3>Log in your first plan</h3>
          <p>Opens the Claude CLI in an isolated config directory for an account named <code>work</code>. Inside, run <code>/login</code>, finish the browser sign-in, then <code>/exit</code>.</p>
          <div class="cmd"><span class="prompt">$</span><code>bun run src/index.ts accounts login work</code><button data-copy>Copy</button></div>
          <p><small>Use any name &mdash; <code>work</code>, <code>personal</code>, <code>team2</code>. Repeat for each plan you want to pool.</small></p>
        </div>
      </div>
      <div class="step">
        <div class="num">3</div>
        <div>
          <h3>Or import an existing login</h3>
          <p>Already signed in with <code>claude</code> on this machine? Copy that login into the pool &mdash; no re-auth needed.</p>
          <div class="cmd"><span class="prompt">$</span><code>bun run src/index.ts accounts import primary</code><button data-copy>Copy</button></div>
        </div>
      </div>
      <div class="step">
        <div class="num">4</div>
        <div>
          <h3>Send a request</h3>
          <p>Once an account is authenticated this page fills in on its own and the endpoints go live.</p>
          <div class="cmd"><span class="prompt">$</span><code>curl http://localhost:PORT_PLACEHOLDER/v1/chat/completions -H "content-type: application/json" -d '{"model":"sonnet","messages":[{"role":"user","content":"Hello!"}]}'</code><button data-copy>Copy</button></div>
          <p><small>Works with any OpenAI or Anthropic client &mdash; point it at <code>/v1</code>.</small></p>
        </div>
      </div>
    </div>
    <div class="waiting"><span class="live"></span>Watching for accounts &mdash; this page refreshes automatically.</div>
  </section>
</main>
<footer>
  <code>/v1/chat/completions</code> <code>/v1/messages</code> <code>/v1/models</code> <code>/api/status</code>
</footer>
<script>
// Theme: follow system by default; toggle overrides and persists.
(function () {
  const saved = localStorage.getItem("cmp-theme");
  if (saved) document.documentElement.setAttribute("data-theme", saved);
  const btn = document.getElementById("theme-toggle");
  btn.addEventListener("click", () => {
    const cur = document.documentElement.getAttribute("data-theme");
    const sysDark = matchMedia("(prefers-color-scheme: dark)").matches;
    const next = cur ? (cur === "dark" ? "light" : "dark") : (sysDark ? "light" : "dark");
    document.documentElement.setAttribute("data-theme", next);
    localStorage.setItem("cmp-theme", next);
  });
})();

const fmtInt = (n) => (n ?? 0).toLocaleString();
const fmtUsd = (n) => "$" + (n ?? 0).toFixed(4);
function timeUntil(ts) {
  if (!ts) return "–";
  const d = ts - Date.now();
  if (d <= 0) return "now";
  const m = Math.round(d / 60000);
  if (m < 60) return m + "m";
  const h = Math.floor(m / 60); return h + "h " + (m % 60) + "m";
}
function ago(ts) {
  if (!ts) return "never";
  const m = Math.round((Date.now() - ts) / 60000);
  if (m < 1) return "just now";
  if (m < 60) return m + "m ago";
  const h = Math.floor(m / 60); return h + "h ago";
}
function esc(s) { return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])); }

function card(a) {
  const dot = a.available ? "ok" : (a.authenticated ? "warn" : "err");
  const state = a.available ? "Ready" : (a.authenticated ? "Sidelined" : "Logged out");
  const u = a.usage;
  const reqPct = Math.min(100, (u.windowRequests / 200) * 100);
  const tok = u.windowInputTokens + u.windowOutputTokens;
  const tokPct = Math.min(100, tok / 1000000 * 100);
  const note = a.unavailableReason
    ? '<div class="note ' + (a.authenticated ? "" : "err") + '">' + esc(a.unavailableReason) + "</div>"
    : (u.lastError ? '<div class="note">Last error: ' + esc(u.lastError) + "</div>" : "");
  const cooldownRow = (u.rateLimitedUntil && u.rateLimitedUntil > Date.now())
    ? '<span class="k">Cooldown</span><span class="v">' + timeUntil(u.rateLimitedUntil) + "</span>" : "";
  return \`<div class="card \${a.available ? "" : "down"}">
    <div class="card-top">
      <span class="status"><span class="dot \${dot}"></span><span class="acct-name">\${esc(a.name)}</span></span>
      <span class="chip plan">\${esc(a.subscriptionType || "unknown")}</span>
    </div>
    <div class="dl">
      <span class="k">Status</span><span class="v">\${state}</span>
      <span class="k">Rate tier</span><span class="v">\${esc(a.rateLimitTier || "–")}</span>
      <span class="k">Token</span><span class="v">\${a.tokenExpired ? "auto-refreshing" : "valid · " + timeUntil(a.tokenExpiresAt)}</span>
      <span class="k">Requests (window)</span><span class="v">\${fmtInt(u.windowRequests)}</span>
      <span class="k">Cost (window)</span><span class="v">\${fmtUsd(u.windowCostUsd)}</span>
      <span class="k">Total requests</span><span class="v">\${fmtInt(u.totalRequests)}</span>
      <span class="k">Last used</span><span class="v">\${ago(u.lastUsedAt)}</span>
      \${cooldownRow}
    </div>
    <div class="bars">
      <div>
        <div class="bar-label"><span>Window load</span><span class="num">\${fmtInt(u.windowRequests)} req</span></div>
        <div class="bar"><span style="width:\${reqPct}%"></span></div>
      </div>
      <div>
        <div class="bar-label"><span>Window tokens</span><span class="num">\${fmtInt(tok)}</span></div>
        <div class="bar"><span style="width:\${tokPct}%"></span></div>
      </div>
    </div>
    \${note}
  </div>\`;
}

// Fill the real origin into the walkthrough's curl example, wire copy buttons.
document.querySelectorAll(".cmd code").forEach((el) => {
  el.textContent = el.textContent.replace("http://localhost:PORT_PLACEHOLDER", location.origin);
});
document.querySelectorAll("[data-copy]").forEach((btn) => {
  btn.addEventListener("click", () => {
    const code = btn.parentElement.querySelector("code").textContent;
    navigator.clipboard?.writeText(code).then(() => {
      const prev = btn.textContent; btn.textContent = "Copied";
      setTimeout(() => (btn.textContent = prev), 1200);
    });
  });
});

async function refresh() {
  try {
    const r = await fetch("/api/status");
    const d = await r.json();
    const grid = document.getElementById("grid");
    const onboard = document.getElementById("onboard");
    const banner = document.getElementById("banner");
    const accounts = d.accounts || [];
    const avail = accounts.filter((a) => a.available).length;

    if (accounts.length === 0) {
      grid.innerHTML = "";
      onboard.style.display = "block";
      banner.style.display = "none";
    } else {
      onboard.style.display = "none";
      grid.innerHTML = accounts.map(card).join("");
      if (avail === 0) {
        const anyAuthed = accounts.some((a) => a.authenticated);
        banner.innerHTML = anyAuthed
          ? "<b>No accounts are available right now.</b> Every account is rate-limited or its token needs attention &mdash; requests will 503 until one frees up. See the cards below."
          : "<b>No accounts are logged in.</b> Authenticate one to start serving requests: <code>bun run src/index.ts accounts login &lt;name&gt;</code>";
        banner.style.display = "block";
      } else {
        banner.style.display = "none";
      }
    }

    document.getElementById("p-accounts").innerHTML = "accounts <b>" + accounts.length + "</b>";
    document.getElementById("p-available").innerHTML = "available <b>" + avail + "</b>";
    document.getElementById("p-window").innerHTML = "window <b>" + Math.round((d.usageWindowMs || 0) / 3600000) + "h</b>";
    document.getElementById("p-updated").textContent = "updated " + new Date().toLocaleTimeString();
  } catch (e) {
    document.getElementById("p-updated").textContent = "offline";
  }
}
refresh();
setInterval(refresh, 4000);
</script>
</body>
</html>`;
}
