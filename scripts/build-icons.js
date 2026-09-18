#!/usr/bin/env node
// Rebuild src/icons/{claude,codex}.png from the installed Claude and Codex
// desktop apps (Windows packages), so the menus show the apps' own marks.
//
//   node scripts/build-icons.js
//
// Each icon is the app's mark alone, in white on a clear background: Claude's
// starburst (its dark-theme tray icon) and Codex's knot (its unplated tile
// logo). They're cropped to the mark — at one text row tall every pixel
// counts — and centred on a 64×64 canvas with just a hairline of clear margin,
// so they fill their row. icons.js recolours them for light terminals.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { decodePng, encodePng, resizeImage } from '../src/term-images.js';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OUT_DIR = path.join(ROOT, 'src', 'icons');
const SIZE = 64;
const MARGIN = 1;

function packageDir(name) {
  const out = execFileSync('powershell.exe', [
    '-NoProfile', '-Command', `(Get-AppxPackage -Name '${name}' | Select-Object -First 1).InstallLocation`
  ], { encoding: 'utf8' }).trim();
  if (!out) throw new Error(`${name} is not installed`);
  return out;
}

// The box around every pixel that shows at all.
function visibleBounds(image) {
  let left = image.width, top = image.height, right = -1, bottom = -1;
  for (let y = 0; y < image.height; y++) {
    for (let x = 0; x < image.width; x++) {
      if (image.data[(y * image.width + x) * 4 + 3] < 16) continue;
      left = Math.min(left, x);
      right = Math.max(right, x);
      top = Math.min(top, y);
      bottom = Math.max(bottom, y);
    }
  }
  return { left, top, width: right - left + 1, height: bottom - top + 1 };
}

// The mark, pure white, scaled to fit inside the margin and centred.
function whiteMark(image) {
  const bounds = visibleBounds(image);
  const cropped = { width: bounds.width, height: bounds.height, data: Buffer.alloc(bounds.width * bounds.height * 4) };
  for (let y = 0; y < bounds.height; y++) {
    const from = ((bounds.top + y) * image.width + bounds.left) * 4;
    image.data.copy(cropped.data, y * bounds.width * 4, from, from + bounds.width * 4);
  }
  const inner = SIZE - MARGIN * 2;
  const scale = inner / Math.max(bounds.width, bounds.height);
  const width = Math.round(bounds.width * scale);
  const height = Math.round(bounds.height * scale);
  const scaled = resizeImage(cropped, width, height);
  const data = Buffer.alloc(SIZE * SIZE * 4);
  const left = Math.floor((SIZE - width) / 2);
  const top = Math.floor((SIZE - height) / 2);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const o = ((top + y) * SIZE + left + x) * 4;
      data[o] = data[o + 1] = data[o + 2] = 255;
      data[o + 3] = scaled.data[(y * width + x) * 4 + 3];
    }
  }
  return { width: SIZE, height: SIZE, data };
}

const sources = {
  claude: path.join(packageDir('Claude'), 'app', 'resources', 'TrayIconLinux-Dark.png'),
  codex: path.join(packageDir('OpenAI.Codex'), 'assets', 'Square44x44Logo.targetsize-256_altform-unplated.png')
};

fs.mkdirSync(OUT_DIR, { recursive: true });
for (const [name, source] of Object.entries(sources)) {
  const file = path.join(OUT_DIR, `${name}.png`);
  fs.writeFileSync(file, encodePng(whiteMark(decodePng(fs.readFileSync(source)))));
  console.log(`${path.relative(ROOT, file)}  ${SIZE}×${SIZE}  (from ${path.basename(source)})`);
}
