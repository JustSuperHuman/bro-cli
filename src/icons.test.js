import { afterEach, expect, test } from 'bun:test';
import { EventEmitter } from 'node:events';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { ICONS, iconModeFromEnv, iconNames, renderIcons, setIconMode } from './icons.js';
import { decodePng, probeTerminal, resizeImage, sixelImage } from './term-images.js';
import { fit, renderLabel } from './ui.js';

const ICON_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'icons');
const plain = (value) => value.replace(/\x1b\[[0-9;]*m/g, '');

afterEach(() => setIconMode(null));

test('an icon is two cells to the layout: truncation and padding keep the columns straight', () => {
  const label = `${ICONS.claude} 37%`;
  // Two cells of icon, a space and "37%": six in all, padded to eight.
  expect(plain(fit(label, 8))).toBe(`${ICONS.claude} 37%  `);
  // Too narrow: the icon goes whole rather than leaving half of it behind.
  expect(plain(fit(`ab${ICONS.codex}`, 3))).toBe('ab…');
  expect(renderLabel({ label: () => label }, 10)).toBe(label);
});

test('without images, each app shows its own terminal mark in bold', () => {
  setIconMode('text');
  const shown = renderIcons(`${ICONS.claude} 37%   ${ICONS.codex} 5%`);
  expect(plain(shown)).toBe('✻  37%   >_ 5%');
  expect(shown).toBe('\x1b[1m✻ \x1b[22m 37%   \x1b[1m>_\x1b[22m 5%');
  // Before the terminal has been asked, menus get the same marks.
  setIconMode(null);
  expect(plain(renderIcons(ICONS.codex))).toBe('>_');
  expect(iconNames(`${ICONS.claude} · ${ICONS.codex}`)).toBe('Claude · Codex');
  expect(renderIcons('no icons here')).toBe('no icons here');
});

test('on a sixel terminal the icon is drawn in place and the cursor steps over its two cells', () => {
  setIconMode('sixel', { cell: { width: 10, height: 20 } });
  const shown = renderIcons(`a${ICONS.claude}b`);
  expect(shown.startsWith('a\x1b7\x1bP0;1;0q"1;1;20;20')).toBe(true);
  expect(shown.endsWith('\x1b\\\x1b8\x1b[2Cb')).toBe(true);
  // Encoded once, reused on every repaint.
  expect(renderIcons(ICONS.claude)).toBe(renderIcons(ICONS.claude));
});

test('iTerm2 gets the PNG sized in cells, kitty an upload once and then placeholders', () => {
  setIconMode('iterm');
  expect(renderIcons(ICONS.codex)).toMatch(/^\x1b7\x1b\]1337;File=inline=1;size=\d+;width=2;height=1;preserveAspectRatio=1:[A-Za-z0-9+/=]+\x07\x1b8\x1b\[2C$/);

  setIconMode('kitty');
  const first = renderIcons(ICONS.codex);
  const again = renderIcons(ICONS.codex);
  expect(first).toContain('\x1b_Ga=T,U=1,f=100,i=202,c=2,r=1,q=2');
  expect(again).not.toContain('\x1b_G');
  expect([...plain(again)].filter((char) => char === '\u{10EEEE}')).toHaveLength(2);
});

test('the terminal is recognised from the environment where it can be, and asked otherwise', () => {
  expect(iconModeFromEnv({ TERM: 'xterm-kitty' })).toBe('kitty');
  expect(iconModeFromEnv({ TERM_PROGRAM: 'ghostty' })).toBe('kitty');
  expect(iconModeFromEnv({ TERM_PROGRAM: 'iTerm.app' })).toBe('iterm');
  expect(iconModeFromEnv({ TERM_PROGRAM: 'WezTerm' })).toBe('iterm');
  expect(iconModeFromEnv({ TMUX: '/tmp/tmux-1/default,1,0', TERM_PROGRAM: 'iTerm.app' })).toBe('text');
  expect(iconModeFromEnv({ WT_SESSION: 'abc' })).toBe('probe');
  expect(iconModeFromEnv({ BRO_ICONS: 'off', TERM: 'xterm-kitty' })).toBe('text');
  expect(iconModeFromEnv({ BRO_ICONS: 'sixel' })).toBe('sixel');
});

function fakeTerminal(reply) {
  const input = new EventEmitter();
  input.isRaw = false;
  input.setRawMode = (on) => { input.isRaw = on; };
  input.resume = () => {};
  input.pause = () => {};
  const output = { written: '', write(text) { this.written += text; if (reply) queueMicrotask(() => input.emit('data', Buffer.from(reply, 'latin1'))); } };
  return { input, output };
}

test('asking the terminal: background from OSC 11, cell size from XTWINOPS, sixel from DA1, and no reply is not an error', async () => {
  const windowsTerminal = fakeTerminal('\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\\x1b[6;20;10t\x1b[?61;4;6;7;14;21;22;23;24;28;32;42c');
  expect(await probeTerminal(windowsTerminal)).toEqual({ sixel: true, cell: { width: 10, height: 20 }, background: [12, 12, 12] });
  expect(windowsTerminal.output.written).toBe('\x1b]11;?\x1b\\\x1b[16t\x1b[c');
  expect(windowsTerminal.input.isRaw).toBe(false);

  expect(await probeTerminal(fakeTerminal('\x1b]11;rgb:ff/ff/ff\x07\x1b[?1;2c'))).toEqual({ sixel: false, cell: null, background: [255, 255, 255] });
  expect(await probeTerminal({ ...fakeTerminal(null), timeoutMs: 20 })).toEqual({ sixel: false, cell: null, background: null });
});

// The palette entries of a sixel image, as [r, g, b] percentages.
const palette = (sixel) => [...sixel.matchAll(/#\d+;2;(\d+);(\d+);(\d+)/g)].map((m) => m.slice(1).map(Number));

test('the marks are white on a dark background, blended into it at the edges, and black on a light one', () => {
  setIconMode('sixel', { cell: { width: 10, height: 20 }, background: [12, 12, 12] });
  const dark = palette(renderIcons(ICONS.claude));
  // The commonest colour is the mark itself, pure white; the rest are its
  // edges blended towards the terminal's own background.
  expect(dark[0]).toEqual([100, 100, 100]);
  expect(dark.every(([r, g, b]) => r === g && g === b)).toBe(true);
  expect(Math.min(...dark.map(([r]) => r))).toBeLessThan(60);

  // On a light one the ink turns black. (Codex's thin strokes are mostly edge
  // at this size, so even its commonest colour is a near-black blend.)
  setIconMode('sixel', { cell: { width: 10, height: 20 }, background: [250, 250, 250] });
  expect(Math.max(...palette(renderIcons(ICONS.codex))[0])).toBeLessThan(25);
  expect(palette(renderIcons(ICONS.claude))[0]).toEqual([0, 0, 0]);

  // Unknown background: white, with the soft edge left to the transparency cutoff.
  setIconMode('sixel', { cell: { width: 10, height: 20 } });
  expect(palette(renderIcons(ICONS.codex))).toEqual([[100, 100, 100]]);
});

test('the bundled app icons are white marks on a clear background', () => {
  for (const name of ['claude', 'codex']) {
    const image = decodePng(fs.readFileSync(path.join(ICON_DIR, `${name}.png`)));
    expect(image.width).toBe(64);
    const visible = [];
    for (let o = 0; o < image.data.length; o += 4) if (image.data[o + 3]) visible.push(o);
    expect(visible.length).toBeGreaterThan(500);
    expect(visible.every((o) => image.data[o] === 255 && image.data[o + 1] === 255 && image.data[o + 2] === 255)).toBe(true);
    // A clear margin all round, so icons on consecutive rows never touch.
    expect(image.data[3]).toBe(0);
    expect(image.data[(64 * 64 - 1) * 4 + 3]).toBe(0);
    const sixel = sixelImage(resizeImage(image, 20, 20));
    expect(sixel).toMatch(/^\x1bP0;1;0q"1;1;20;20#0;2;\d+;\d+;\d+/);
    expect(sixel.endsWith('\x1b\\')).toBe(true);
  }
});
