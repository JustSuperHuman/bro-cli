// App icons in menu text.
//
// A label carries an icon as one placeholder character (`ICONS.claude`), which
// the layout code counts as two cells wide — the size every icon is drawn at,
// one text row tall. Truncation, alignment, highlighting and filtering all
// work on that placeholder like on any other wide character; only when a frame
// is written to the terminal does renderIcons() turn each one into the real
// thing for that terminal:
//
//   sixel / iterm / kitty — the app's own mark (src/icons/*.png), white, or
//                           black on a light background; see term-images.js
//                           for which terminals speak which
//   text                  — the app's own terminal mark in bold: Claude
//                           Code's ✻ and Codex CLI's >_
//
// The mode is detected once per run (detectIconMode) before the first menu
// takes the keyboard; BRO_ICONS=sixel|iterm|kitty|text forces one.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  decodePng,
  encodePng,
  itermImage,
  kittyPlaceholder,
  kittyUpload,
  probeTerminal,
  resizeImage,
  sixelImage
} from './term-images.js';

const ICON_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'icons');

// Supplementary Private Use Area code points: never in real text (Nerd Fonts
// stop at plane 15's start), and already two cells wide to the layout code.
export const ICONS = {
  claude: '\u{10FF00}',
  codex: '\u{10FF01}'
};
export const ICON_WIDTH = 2;

const APPS = {
  [ICONS.claude]: { name: 'claude', label: 'Claude', glyph: '✻ ', kittyId: 201 },
  [ICONS.codex]: { name: 'codex', label: 'Codex', glyph: '>_', kittyId: 202 }
};
const ICON_PATTERN = /[\u{10FF00}\u{10FF01}]/gu;
const HAS_ICON = /[\u{10FF00}\u{10FF01}]/u;

export const isIcon = (codePoint) => codePoint === 0x10ff00 || codePoint === 0x10ff01;

// --- what this terminal can show ----------------------------------------------

const MODES = ['sixel', 'iterm', 'kitty', 'text'];
let mode = null;
let cellPixels = null;
let background = null;
let detecting = null;

// What the environment alone tells about the terminal: a mode, or 'probe'
// when only asking the terminal can tell (sixel support is not advertised in
// the environment).
export function iconModeFromEnv(env = process.env) {
  const forced = String(env.BRO_ICONS || '').toLowerCase();
  if (MODES.includes(forced)) return forced;
  if (forced === 'off' || forced === 'none' || forced === '0') return 'text';
  // A multiplexer sits between us and the terminal and would need every
  // image wrapped in its own passthrough.
  if (env.TMUX || /^screen/.test(env.TERM || '')) return 'text';
  if (env.KITTY_WINDOW_ID || env.TERM === 'xterm-kitty' || env.TERM_PROGRAM === 'ghostty' || env.TERM === 'xterm-ghostty') return 'kitty';
  if (env.TERM_PROGRAM === 'iTerm.app' || env.LC_TERMINAL === 'iTerm2' || env.TERM_PROGRAM === 'WezTerm') return 'iterm';
  return 'probe';
}

// Settle the mode for this run. Must finish before a menu starts reading
// keys: the terminal's answers arrive on stdin. Any terminal that may draw
// images is asked once — for sixel support, its cell size and its background
// colour, which decides how the white marks are drawn.
export function detectIconMode({ input = process.stdin, output = process.stdout, env = process.env } = {}) {
  if (mode) return Promise.resolve(mode);
  detecting ||= (async () => {
    let found = iconModeFromEnv(env);
    if (found !== 'text' && input.isTTY && output.isTTY && typeof input.setRawMode === 'function') {
      const answer = await probeTerminal({ input, output });
      background = answer.background;
      cellPixels = answer.cell;
      if (found === 'probe') found = answer.sixel ? 'sixel' : 'text';
    }
    if (found === 'probe') found = 'text';
    // Windows Terminal draws sixels as if every cell were 10×20 pixels,
    // whatever the font, and scales them to the real cell (it also reports
    // that size when asked).
    if (found === 'sixel' && (env.WT_SESSION || !cellPixels)) cellPixels = { width: 10, height: 20 };
    mode = found;
    return mode;
  })();
  return detecting;
}

export function setIconMode(next, { cell = null, background: color = null } = {}) {
  mode = next;
  cellPixels = cell;
  background = color;
  detecting = null;
  rendered.clear();
  kittyUploaded.clear();
}

// --- drawing ------------------------------------------------------------------

const rendered = new Map();
const kittyUploaded = new Set();

const luminance = ([r, g, b]) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;

// The marks are white; on a light background white would vanish, so there
// they're drawn black instead.
const inkColor = () => (background && luminance(background) > 0.5 ? [0, 0, 0] : [255, 255, 255]);

// The mark at `width` × `height` pixels in `ink`. Sixel has no partial
// transparency, so when the background is known the soft edge pixels are
// blended with it here and drawn solid; unknown, they're left to the cutoff.
// At one text row, Codex's strokes are thinner than a pixel and would come
// out grey, so for sixel (`blend`) the edges are strengthened a little first.
const STROKE_GAMMA = 0.7;

function markPixels(app, width, height, { blend }) {
  const image = resizeImage(decodePng(fs.readFileSync(path.join(ICON_DIR, `${app.name}.png`))), width, height);
  const ink = inkColor();
  for (let o = 0; o < image.data.length; o += 4) {
    const alpha = blend ? (image.data[o + 3] / 255) ** STROKE_GAMMA : image.data[o + 3] / 255;
    if (blend && !background) image.data[o + 3] = Math.round(alpha * 255);
    for (let c = 0; c < 3; c++) {
      image.data[o + c] = blend && background ? Math.round(ink[c] * alpha + background[c] * (1 - alpha)) : ink[c];
    }
    if (blend && background) image.data[o + 3] = alpha >= 0.08 ? 255 : 0;
  }
  return image;
}

// The mark as a PNG in the right ink, at its bundled size, for protocols that
// let the terminal scale and blend it.
function markPng(app) {
  const file = path.join(ICON_DIR, `${app.name}.png`);
  if (inkColor()[0] === 255) return fs.readFileSync(file);
  const image = decodePng(fs.readFileSync(file));
  return encodePng(markPixels(app, image.width, image.height, { blend: false }));
}

// The app's terminal mark, two cells wide, in bold — white on a dark theme,
// and whatever the text colour is on any other.
function textIcon(app) {
  return `\x1b[1m${app.glyph}\x1b[22m`;
}

// Draw an image at the cursor without trusting where the terminal leaves the
// cursor afterwards (sixel moves it down, iTerm2 by the image's width): save
// it, draw, restore, and step over the icon's two cells.
const inPlace = (image) => `\x1b7${image}\x1b8\x1b[${ICON_WIDTH}C`;

function imageIcon(app) {
  let image = rendered.get(app.name);
  if (image === undefined) {
    try {
      if (mode === 'sixel') {
        const { width, height } = cellPixels;
        image = inPlace(sixelImage(markPixels(app, width * ICON_WIDTH, height, { blend: true })));
      } else if (mode === 'iterm') {
        image = inPlace(itermImage(markPng(app), { columns: ICON_WIDTH, rows: 1 }));
      } else {
        image = kittyPlaceholder({ id: app.kittyId, columns: ICON_WIDTH });
      }
    } catch {
      image = null;
    }
    rendered.set(app.name, image);
  }
  if (image === null) return textIcon(app);
  // Kitty needs the picture once before its placeholders can show it.
  if (mode === 'kitty' && !kittyUploaded.has(app.name)) {
    kittyUploaded.add(app.name);
    return kittyUpload(markPng(app), { id: app.kittyId, columns: ICON_WIDTH, rows: 1 }) + image;
  }
  return image;
}

// Turn icon placeholders in a frame into what this terminal shows. Before the
// mode is known (or away from a terminal) that's the text marks.
export function renderIcons(text) {
  if (!HAS_ICON.test(text)) return text;
  return text.replace(ICON_PATTERN, (token) => {
    const app = APPS[token];
    return !mode || mode === 'text' ? textIcon(app) : imageIcon(app);
  });
}

// Icons as plain words, for text that leaves the terminal (logs, messages).
export function iconNames(text) {
  return text.replace(ICON_PATTERN, (token) => APPS[token].label);
}
