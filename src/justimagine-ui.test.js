import { expect, test } from 'bun:test';
import fs from 'node:fs';
import vm from 'node:vm';
import { UI_HTML } from './justimagine-server.js';

// The gallery is one HTML file with an inline module-less script. Nothing else
// compiles it, so a stray brace would only show up as a blank page in someone's
// browser — these checks are the compile step.
const html = fs.readFileSync(UI_HTML, 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
// Two: a tiny theme bootstrap in <head> that has to run before the first paint,
// and the app itself at the end of <body>.
const [bootstrap, app] = scripts;

test('the page has a head bootstrap and an app script, and both parse', () => {
  expect(scripts.length).toBe(2);
  for (const s of scripts) expect(() => new vm.Script(s)).not.toThrow();
  expect(bootstrap.split('\n').length).toBeLessThan(20); // it blocks paint; keep it tiny
  expect(app.split('\n').length).toBeGreaterThan(400);
});

// A saved dark preference has to be on the html element before the first paint,
// or the page flashes white on every load.
test('the theme bootstrap applies a saved preference before anything renders', () => {
  const head = html.slice(0, html.indexOf('</head>'));
  expect(head).toContain(bootstrap.trim().slice(0, 40));
  expect(bootstrap).toContain("localStorage.getItem('justimagine:v1')");
  expect(bootstrap).toContain("setAttribute('data-theme', 'dark')");
  // storage can throw outright in a locked-down browser; the page must still boot
  expect(bootstrap).toMatch(/try\s*\{[\s\S]*\}\s*catch/);

  // and it must actually work: run it against a fake localStorage
  const run = (stored) => {
    const el = { attrs: {}, setAttribute(k, v) { this.attrs[k] = v; } };
    new Function('localStorage', 'document', bootstrap)(
      { getItem: () => stored },
      { documentElement: el }
    );
    return el.attrs['data-theme'];
  };
  expect(run(JSON.stringify({ theme: 'dark' }))).toBe('dark');
  expect(run(JSON.stringify({ theme: 'light' }))).toBeUndefined();
  expect(run(null)).toBeUndefined(); // nothing saved yet — light is the default
  expect(run('not json')).toBeUndefined();
});

test('every element the script reaches for by id exists in the markup', () => {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  const wanted = new Set([...app.matchAll(/\$\('([A-Za-z0-9_-]+)'\)/g)].map((m) => m[1]));
  const missing = [...wanted].filter((id) => !ids.has(id));
  expect(missing).toEqual([]);
  expect(wanted.size).toBeGreaterThan(20);
});

test('every class the script toggles is styled somewhere', () => {
  // A lookahead, not a consuming match: `.card.sel` has to yield both names.
  const styled = new Set([...html.matchAll(/\.([a-zA-Z][\w-]*)(?=[\s{,:.>[)]|$)/gm)].map((m) => m[1]));
  const toggled = new Set(
    [...app.matchAll(/classList\.(?:add|toggle|remove)\('([a-zA-Z][\w-]*)'/g)].map((m) => m[1])
  );
  expect([...toggled].filter((c) => !styled.has(c))).toEqual([]);
});

// These run while the composer is still being wired up, so every binding they
// read must already be initialised. A `let`/`const` declared further down is a
// temporal dead zone error — and one that only fires on the code path that
// happens to touch it, so a stale preference can blank the page for one user
// and nobody else. (Both real instances of this were exactly that shape.)
const BOOT_TIME_FUNCTIONS = ['persist', 'syncKnobs', 'fillModels', 'renderRefs', 'renderCast', 'renderCastPicker', 'renderTree'];

function bindingsReadBy(body, fnName) {
  const at = body.indexOf(`function ${fnName}(`);
  if (at < 0) return null;
  const end = body.indexOf('\n  }', at);
  // Strip `.property` accesses and `key:` labels; only bare identifiers are
  // reads of an outer binding. The lookbehind keeps `...spread` out of it.
  const bare = body
    .slice(at, end)
    .replace(/(?<!\.)\.\s*[a-zA-Z_$][\w$]*/g, '')
    .replace(/\b[a-zA-Z_$][\w$]*\s*:/g, '');
  return { at, source: body.slice(at, end), names: new Set([...bare.matchAll(/\b([a-zA-Z_$][\w$]*)\b/g)].map((m) => m[1])) };
}

// Only bindings at the script's own top level can be in a dead zone when a
// boot-time function runs; a `const` inside some later function is its own
// local and irrelevant here.
function topLevelBindings(body) {
  const at = new Map();
  for (const m of body.matchAll(/^ {2}(?:let|const)\s+([a-zA-Z_$][\w$]*)([^\n]*)/gm)) {
    if (!at.has(m[1])) at.set(m[1], m.index);
    // `const a = $('a'), b = $('b')` declares more than one name on the line
    for (const extra of m[2].matchAll(/,\s*([a-zA-Z_$][\w$]*)\s*=/g)) {
      if (!at.has(extra[1])) at.set(extra[1], m.index);
    }
  }
  return at;
}

test('every binding the boot path reads is declared before it runs', () => {
  const body = app;
  const declaredAt = topLevelBindings(body);
  expect(declaredAt.size).toBeGreaterThan(20);

  const offenders = [];
  for (const fn of BOOT_TIME_FUNCTIONS) {
    const found = bindingsReadBy(body, fn);
    expect(found).not.toBe(null); // the list must not go stale as the UI changes
    const localTo = new Set([...found.source.matchAll(/\b(?:let|const)\s+([a-zA-Z_$][\w$]*)/g)].map((m) => m[1]));
    for (const name of found.names) {
      if (localTo.has(name)) continue;
      const decl = declaredAt.get(name);
      if (decl !== undefined && decl > found.at) offenders.push(`${fn} reads ${name}`);
    }
  }
  expect(offenders).toEqual([]);
});

// A reference outside the range every upstream accepts is a failed generation,
// not a slightly worse one — Seedance refuses anything under 300px per side.
// This is the shipped function, lifted out of the page and run for real.
test('references are scaled into the range upstreams accept, both directions', () => {
  const body = app;
  const src = body.slice(body.indexOf('function fitForUpstream('), body.indexOf('function fileToDataUrl('));
  const MAX = Number(body.match(/MAX_DIM = (\d+)/)[1]);
  const MIN = Number(body.match(/MIN_DIM = (\d+)/)[1]);
  const fit = new Function('MAX_DIM', 'MIN_DIM', `${src}; return fitForUpstream;`)(MAX, MIN);

  // already in range — left completely alone
  expect(fit(1024, 768)).toBe(null);
  expect(fit(MIN, MIN)).toBe(null);
  expect(fit(MAX, MAX)).toBe(null);

  // the reported failure: 185x151 is under the floor on both sides
  const up = fit(185, 151);
  expect(Math.min(up.w, up.h)).toBe(MIN);
  expect(up.scale).toBeGreaterThan(1);
  // aspect ratio survives
  expect(up.w / up.h).toBeCloseTo(185 / 151, 2);

  // oversized still comes down
  const down = fit(6000, 4000);
  expect(Math.max(down.w, down.h)).toBe(MAX);
  expect(down.scale).toBeLessThan(1);

  // a sliver must not blow past the ceiling on its way up to the floor
  const sliver = fit(4000, 10);
  expect(Math.max(sliver.w, sliver.h)).toBeLessThanOrEqual(MAX);

  // nothing ever collapses to zero
  for (const [w, h] of [[1, 1], [1, 4000], [3, 2]]) {
    const r = fit(w, h);
    expect(r.w).toBeGreaterThan(0);
    expect(r.h).toBeGreaterThan(0);
    expect(Math.max(r.w, r.h)).toBeLessThanOrEqual(MAX);
  }
});

test('the page declares a title and a favicon', () => {
  expect(html).toContain('<title>JustImagine</title>');
  expect(html).toContain('rel="icon"');
});

// Light is the default and dark is opted into — the system preference is
// deliberately not consulted, so the media query must not creep back in.
test('dark mode is an explicit choice, not the system preference', () => {
  expect(html).not.toContain('prefers-color-scheme');
  expect(html).toContain('color-scheme: light;');
  expect(html).toContain("[data-theme='dark']");

  // every token defined for light must be redefined for dark, or something
  // ends up unreadable on the wrong ground
  const tokensIn = (block) => new Set([...block.matchAll(/(--[\w-]+):/g)].map((m) => m[1]));
  const light = html.slice(html.indexOf('  :root {'), html.indexOf("  :root[data-theme='dark']"));
  const darkStart = html.indexOf("  :root[data-theme='dark']");
  const dark = html.slice(darkStart, html.indexOf('\n  }', darkStart));
  // font/shape tokens are theme-independent; colours are not
  const colourish = (t) => !/^--(sans|mono|radius|ease)$/.test(t);
  const missing = [...tokensIn(light)].filter((t) => colourish(t) && !tokensIn(dark).has(t));
  expect(missing).toEqual([]);
});

test('the theme toggle is a real button that writes the preference back', () => {
  expect(html).toContain('id="theme"');
  expect(html).toContain('aria-pressed');
  // one icon each way, and only the one you would switch *to* is shown
  expect(html).toContain('class="sun"');
  expect(html).toContain('class="moon"');
  expect(html).toContain(":root:not([data-theme='dark']) #theme .moon");
  expect(html).toContain(":root[data-theme='dark'] #theme .sun");
  expect(app).toContain("save({ theme: dark ? 'dark' : 'light' })");
  // light is the absence of the attribute, which is what the bootstrap reads
  expect(app).toContain("removeAttribute('data-theme')");
});

test('the composer offers both media kinds and every per-model video control', () => {
  for (const id of ['duration', 'resolution', 'aspect', 'audio', 'seed', 'firstFrame']) {
    expect(html).toContain(`id="${id}"`);
  }
  expect(html).toContain('data-mode="image"');
  expect(html).toContain('data-mode="video"');
  // video knobs are grouped so switching modes shows/hides them as a set
  expect((html.match(/videoOnly/g) || []).length).toBeGreaterThan(5);
  expect((html.match(/imageOnly/g) || []).length).toBeGreaterThan(2);
});

test('both text fields have a magic button and a revert beside them', () => {
  for (const id of ['magic', 'revert', 'edMagic', 'edRevert']) expect(html).toContain(`id="${id}"`);
  // each pair lives in its own wrapper so the buttons sit on the right field
  expect((html.match(/class="prompt-wrap"/g) || []).length).toBe(2);
  expect((html.match(/class="prompt-tools"/g) || []).length).toBe(2);
  // the wiring is shared rather than written twice
  expect((app.match(/wireMagic\(/g) || []).length).toBe(3); // one definition, two uses
  expect(app).toContain("kind: 'character'");
});

test('the editor can draw a reference sheet and pick from what comes back', () => {
  for (const id of ['edDraw', 'edDrawNote', 'edCandField', 'edCands', 'edCandCount', 'edKeep', 'edDiscard']) {
    expect(html).toContain(`id="${id}"`);
  }
  const body = app;
  expect(body).toContain('/api/characters/refs/generate');
  expect(body).toContain('/api/characters/refs/keep');
  expect(body).toContain('/api/characters/candidates/clear');
  // a reference-sheet job belongs to the editor, never the gallery
  expect(body).toMatch(/if \(job\.characterId\) \{[\s\S]{0,120}?onDrawProgress\(job\);[\s\S]{0,40}?continue;/);
  // nothing is kept without being picked
  expect(body).toContain('chosenCands');
});

// A control inside a popup re-renders that popup in its own click handler. By
// the time a document-level *click* listener runs, the clicked element has been
// replaced, `contains(target)` is false, and the popup closes itself — which is
// why clicking a sort pill used to dismiss the model menu. pointerdown fires
// before the re-render, while the target is still attached.
test('popups close on an outside pointerdown, not on click', () => {
  expect(app).toContain("document.addEventListener('pointerdown'");
  expect(app).not.toMatch(/document\.addEventListener\('click',[^\n]*classList\.remove\('open'\)/);
  // both popups go through the one helper
  expect(app).toContain('closeOnOutside(modelMenu, modelBtn)');
  expect(app).toContain('closeOnOutside(castMenu, castBtn)');
});

test('the model picker becomes a full-screen sheet on a phone', () => {
  // the detail pane used to be display:none below 720px, so a phone could see
  // prices but never read what a model was for
  const narrow = html.slice(html.indexOf('@media (max-width: 720px)'));
  expect(narrow).toContain('position: fixed');
  expect(html).toContain('id="mmClose"');
  expect(html).toContain('class="mm-close"');
  expect(app).toContain('sheetMode');
  // a tap previews and the sheet's own button commits, because there is no hover
  expect(app).toMatch(/if \(!sheetMode\(\)\) return chooseModel/);
  // and the page behind the sheet is locked
  expect(app).toContain("classList.toggle('sheet-open', sheetMode())");
  expect(html).toContain('body.sheet-open { overflow: hidden; }');
});

// Speed measurements take a couple of seconds to collect, so the server pushes
// them when they land rather than making the page reload to see them.
test('the page folds in the model refresh the server pushes', () => {
  expect(app).toMatch(/msg\.type === 'models'/);
  expect(app).toContain('refreshModelSources()');
  // the lists are mutated in place, so closures that captured them keep working
  expect(app).toMatch(/videoModels\.length = 0;[\s\S]{0,120}?imageApis\.length = 0;/);
  // and an open picker repaints instead of going stale
  expect(app).toMatch(/if \(modelMenu\.classList\.contains\('open'\)\) renderModelMenu\(\)/);
});

test('the picker credits OpenRouter for the speed it shows', () => {
  expect(app).toContain("speed: OpenRouter's median generation time, last 30 minutes");
  // the old wording survives only as the fallback for a model with no traffic
  expect(app).toContain('speed: median of your own generations here');
  // a fact nobody in the list has is left out rather than shown as dashes
  expect(app).toMatch(/const availableFacts = \(list\) => \{[\s\S]{0,300}?'age', 'cost', 'speed', 'quality'/);
});

test('the character editor takes pasted and dropped images', () => {
  const body = app;
  expect(body).toContain('addEditorRefs');
  // scoped to the open editor and captured, so a paste does not also land in
  // the composer's reference library
  expect(body).toMatch(/editorEl\.classList\.contains\('open'\)[\s\S]{0,400}?stopImmediatePropagation/);
  expect(body).toMatch(/addEventListener\('paste'[\s\S]{0,600}?\}, true\)/);
  for (const ev of ['dragover', 'dragleave', 'drop']) expect(body).toContain(`editorEl.addEventListener('${ev}'`);
  expect(html).toContain('id="edDrop"');
});

test('the lightbox can show a video, not just an image', () => {
  expect(html).toContain('id="lbVid"');
  expect(html).toContain('<video id="lbVid" controls playsinline');
  expect(app).toContain('lbVid.poster');
});

test('the page talks to the routes the server actually serves', () => {
  const called = new Set([...app.matchAll(/['"`](\/api\/[a-z/-]+)/g)].map((m) => m[1]));
  const server = fs.readFileSync(new URL('../vendor/justimagine/src/justimagine-server.js', import.meta.url), 'utf8');
  const served = new Set([
    ...[...server.matchAll(/'(?:GET|POST) (\/api\/[a-z/-]+)'/g)].map((m) => m[1]),
    '/api/events'
  ]);
  expect([...called].filter((p) => !served.has(p))).toEqual([]);
  expect(called.has('/api/generate')).toBe(true);
  expect(called.has('/api/events')).toBe(true);
});

// The bug this replaced: the menu was aligned to the button's left edge and
// "flipped" to its right edge when that overflowed. A button in the middle of a
// wide toolbar has room on neither side, so both alignments ran off screen —
// and because the price sits at the right end of every row, running off the
// right edge is exactly what hid the cost column. The geometry is pure, so it
// is checked here at every window size rather than at whichever one was open.
const modelMenuBox = (() => {
  const src = app.match(/ {2}function modelMenuBox\([\s\S]*?\n {2}\}/);
  if (!src) throw new Error('modelMenuBox not found in the app script');
  return new Function(`${src[0]}\nreturn modelMenuBox;`)();
})();

// A 44px-tall button 100px down a 900px-tall window, unless stated otherwise.
const at = (pickLeft, viewportWidth, extra = {}) =>
  modelMenuBox({ pickLeft, viewportWidth, viewportHeight: 900, pickTop: 100, pickBottom: 144, ...extra });

test('the model menu is placed fully on screen at every window width', () => {
  const edge = 12;
  // Every combination of window width and button position must land the menu
  // entirely inside the viewport — that is the whole contract.
  for (const viewportWidth of [320, 500, 721, 760, 900, 1024, 1280, 1440, 1920, 2560]) {
    for (const pickLeft of [0, 12, 200, 480, 900, 1500, 2400]) {
      if (pickLeft > viewportWidth) continue;
      const box = at(pickLeft, viewportWidth);
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.left + box.width).toBeLessThanOrEqual(viewportWidth);
      // `offset` is what gets written to style.left, relative to the button.
      expect(box.left).toBe(pickLeft + box.offset);
    }
  }
});

test('the menu sits under the button when it fits, and slides back only as far as it must', () => {
  // Room to spare: aligned to the button, no nudging.
  const roomy = at(200, 1920);
  expect(roomy.width).toBe(880);
  expect(roomy.left).toBe(200);
  expect(roomy.offset).toBe(0);

  // Button far right: slid left to sit against the right margin, and no further.
  const right = at(1500, 1920);
  expect(right.left).toBe(1920 - 12 - 880);
  expect(right.offset).toBe(right.left - 1500);
  expect(right.offset).toBeLessThan(0);

  // Button mid-toolbar on a medium window — the case the old flip got wrong in
  // both directions.
  const middle = at(480, 1024);
  expect(middle.width).toBe(880); // the 880 cap, not the 1000 available
  expect(middle.left).toBe(1024 - 12 - 880);
  expect(middle.left + middle.width).toBe(1012);
  // Slid left of the button rather than off the right edge, which is what the
  // flip did — taking the price column with it.
  expect(middle.offset).toBeLessThan(0);

  // Narrower than the menu's maximum: it takes the width available.
  expect(at(0, 500).width).toBe(500 - 24);
  // Never narrower than something usable, even if that means overhanging.
  expect(at(0, 200).width).toBe(280);
});

test('the menu stacks its columns once it is too narrow for two', () => {
  // The stacking threshold is the menu's own width, not the window's.
  expect(at(0, 1920).width).toBeGreaterThanOrEqual(700);
  expect(at(0, 640).width).toBeLessThan(700);
});

test('the menu opens upward only when downward is genuinely too short', () => {
  // Plenty of room below: downward, and the height is what is left below it.
  const down = modelMenuBox({ pickLeft: 20, viewportWidth: 1440, viewportHeight: 900, pickTop: 100, pickBottom: 144 });
  expect(down.up).toBe(false);
  expect(down.height).toBe(900 - 144 - 12);

  // A short window with the button low: upward, sized to the room above.
  const up = modelMenuBox({ pickLeft: 20, viewportWidth: 1440, viewportHeight: 620, pickTop: 420, pickBottom: 464 });
  expect(up.up).toBe(true);
  expect(up.height).toBe(420 - 12);

  // Cramped both ways: still opens the way with more room, and stays usable.
  const tight = modelMenuBox({ pickLeft: 20, viewportWidth: 1440, viewportHeight: 300, pickTop: 240, pickBottom: 284 });
  expect(tight.up).toBe(true);
  expect(tight.height).toBeGreaterThanOrEqual(200);
});
