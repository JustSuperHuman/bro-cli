import { expect, test } from 'bun:test';
import fs from 'node:fs';
import vm from 'node:vm';
import { UI_HTML } from './justimagine-server.js';

// The gallery is one HTML file with an inline module-less script. Nothing else
// compiles it, so a stray brace would only show up as a blank page in someone's
// browser — these checks are the compile step.
const html = fs.readFileSync(UI_HTML, 'utf8');
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)].map((m) => m[1]);

test('the page has exactly one inline script and it parses', () => {
  expect(scripts.length).toBe(1);
  expect(() => new vm.Script(scripts[0])).not.toThrow();
  expect(scripts[0].split('\n').length).toBeGreaterThan(400);
});

test('every element the script reaches for by id exists in the markup', () => {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  const wanted = new Set([...scripts[0].matchAll(/\$\('([A-Za-z0-9_-]+)'\)/g)].map((m) => m[1]));
  const missing = [...wanted].filter((id) => !ids.has(id));
  expect(missing).toEqual([]);
  expect(wanted.size).toBeGreaterThan(20);
});

test('every class the script toggles is styled somewhere', () => {
  // A lookahead, not a consuming match: `.card.sel` has to yield both names.
  const styled = new Set([...html.matchAll(/\.([a-zA-Z][\w-]*)(?=[\s{,:.>[)]|$)/gm)].map((m) => m[1]));
  const toggled = new Set(
    [...scripts[0].matchAll(/classList\.(?:add|toggle|remove)\('([a-zA-Z][\w-]*)'/g)].map((m) => m[1])
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
  const body = scripts[0];
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
  const body = scripts[0];
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

test('the page declares a title, a favicon and both colour schemes', () => {
  expect(html).toContain('<title>JustImagine</title>');
  expect(html).toContain('rel="icon"');
  expect(html).toContain('@media (prefers-color-scheme: dark)');
  expect(html).toContain('color-scheme: light dark');
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
  expect((scripts[0].match(/wireMagic\(/g) || []).length).toBe(3); // one definition, two uses
  expect(scripts[0]).toContain("kind: 'character'");
});

test('the editor can draw a reference sheet and pick from what comes back', () => {
  for (const id of ['edDraw', 'edDrawNote', 'edCandField', 'edCands', 'edCandCount', 'edKeep', 'edDiscard']) {
    expect(html).toContain(`id="${id}"`);
  }
  const body = scripts[0];
  expect(body).toContain('/api/characters/refs/generate');
  expect(body).toContain('/api/characters/refs/keep');
  expect(body).toContain('/api/characters/candidates/clear');
  // a reference-sheet job belongs to the editor, never the gallery
  expect(body).toMatch(/if \(job\.characterId\) \{[\s\S]{0,120}?onDrawProgress\(job\);[\s\S]{0,40}?continue;/);
  // nothing is kept without being picked
  expect(body).toContain('chosenCands');
});

test('the character editor takes pasted and dropped images', () => {
  const body = scripts[0];
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
  expect(scripts[0]).toContain('lbVid.poster');
});

test('the page talks to the routes the server actually serves', () => {
  const called = new Set([...scripts[0].matchAll(/['"`](\/api\/[a-z/-]+)/g)].map((m) => m[1]));
  const server = fs.readFileSync(new URL('./justimagine-server.js', import.meta.url), 'utf8');
  const served = new Set([
    ...[...server.matchAll(/'(?:GET|POST) (\/api\/[a-z/-]+)'/g)].map((m) => m[1]),
    '/api/events'
  ]);
  expect([...called].filter((p) => !served.has(p))).toEqual([]);
  expect(called.has('/api/generate')).toBe(true);
  expect(called.has('/api/events')).toBe(true);
});
