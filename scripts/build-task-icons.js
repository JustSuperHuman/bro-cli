#!/usr/bin/env node
// Build the four task-launcher icons in assets/icons — the ones a Windows
// Terminal profile, a shortcut or a taskbar tile uses to open a login chosen
// by what it has left (src/task-size.js):
//
//   claude-big-task    bro --large-task          Claude, most allowance left
//   claude-small-task  bro --small-task          Claude, least that can finish
//   codex-big-task     bro codex --large-task    Codex, most left
//   codex-small-task   bro codex --small-task    Codex, least that can finish
//
//   node scripts/build-task-icons.js
//
// Each icon is the app's own mark, white on the app's own plate — Claude's
// clay, Codex's black — over a band across the foot of the tile saying which
// way the flag chooses.
//
// The band is the colour code, and it is the whole point of the design: at
// the 16 px a taskbar actually draws, a corner badge is one grey dot and the
// mark is a smudge, so nothing but a large block of colour survives. It uses
// the palette bro's own meters use (usage.js): green where there is room,
// red where there is nearly none — so the big-task icon is green and the
// small-task icon red, which is also what their logins' figures would be
// coloured. The arrow in the band and the mark's size repeat the same thing
// in shape, for anyone who doesn't read the pair as two colours.
//
// Everything but the mark is drawn at 4x and averaged down (resizeImage is
// an area resampler), which is where the anti-aliasing comes from; the mark
// is a photograph of someone else's logo, so it is resized once, straight to
// the size it is drawn at, rather than up and back down.
//
// Each icon is written twice: a 512 px PNG, and an ICO carrying every size
// Windows asks for (BMP below 256, PNG at 256, which is what the format
// expects).

import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { decodePng, encodePng, resizeImage } from '../src/term-images.js';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const OUT_DIR = path.join(ROOT, 'assets', 'icons');
const PNG_SIZE = 512;
const ICO_SIZES = [16, 24, 32, 48, 64, 128, 256];
const SUPERSAMPLE = 4;

// --- the apps' own marks -----------------------------------------------------

// The installed desktop apps carry their marks at 256 px, white on a clear
// background, which is exactly what goes on the plate. bro's bundled 64 px
// copies (src/icons, for terminal rows) stand in wherever an app isn't
// installed, so this still builds on a machine with neither.
function packageDir(name) {
  try {
    const out = execFileSync('powershell.exe', [
      '-NoProfile', '-Command', `(Get-AppxPackage -Name '${name}' | Select-Object -First 1).InstallLocation`
    ], { encoding: 'utf8' }).trim();
    return out || null;
  } catch {
    return null;
  }
}

// The largest PNG-compressed image in an .ico. Windows tray icons keep their
// 256 px entry this way; the smaller entries are BMP and not worth reading.
function largestPngInIco(file) {
  const buffer = fs.readFileSync(file);
  if (buffer.readUInt16LE(0) !== 0 || buffer.readUInt16LE(2) !== 1) return null;
  const PNG = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  let best = null;
  for (let i = 0; i < buffer.readUInt16LE(4); i++) {
    const entry = 6 + i * 16;
    const width = buffer[entry] || 256;
    const size = buffer.readUInt32LE(entry + 8);
    const offset = buffer.readUInt32LE(entry + 12);
    if (!buffer.subarray(offset, offset + 8).equals(PNG)) continue;
    if (!best || width > best.width) best = { width, data: buffer.subarray(offset, offset + size) };
  }
  return best && decodePng(best.data);
}

function loadMark(app) {
  for (const source of app.sources()) {
    try {
      const image = source.endsWith('.ico') ? largestPngInIco(source) : decodePng(fs.readFileSync(source));
      if (image) return { image, from: path.basename(source) };
    } catch {
      /* try the next place this mark lives */
    }
  }
  const fallback = path.join(ROOT, 'src', 'icons', `${app.name}.png`);
  return { image: decodePng(fs.readFileSync(fallback)), from: `src/icons/${app.name}.png` };
}

// The mark cropped to the pixels that actually show, so a logo with padding
// baked into it fills its share of the plate like one without.
function cropToInk(image) {
  let left = image.width, top = image.height, right = -1, bottom = -1;
  for (let y = 0; y < image.height; y++) {
    for (let x = 0; x < image.width; x++) {
      if (image.data[(y * image.width + x) * 4 + 3] < 16) continue;
      if (x < left) left = x;
      if (x > right) right = x;
      if (y < top) top = y;
      if (y > bottom) bottom = y;
    }
  }
  if (right < left) return image;
  const width = right - left + 1;
  const height = bottom - top + 1;
  const data = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    const from = ((top + y) * image.width + left) * 4;
    image.data.copy(data, y * width * 4, from, from + width * 4);
  }
  return { width, height, data };
}

// --- drawing -----------------------------------------------------------------

const canvas = (size) => ({ width: size, height: size, data: Buffer.alloc(size * size * 4) });

// Paint `color` wherever `inside(x, y)` says, over what is already there.
// Coordinates are the pixel centres, in the canvas's own pixels.
function paint(image, inside, color) {
  for (let y = 0; y < image.height; y++) {
    for (let x = 0; x < image.width; x++) {
      const shade = inside(x + 0.5, y + 0.5);
      if (!shade) continue;
      const [r, g, b, a = 255] = typeof shade === 'object' ? shade : color;
      const o = (y * image.width + x) * 4;
      const alpha = a / 255;
      const behind = (image.data[o + 3] / 255) * (1 - alpha);
      const total = alpha + behind;
      image.data[o] = Math.round((r * alpha + image.data[o] * behind) / total);
      image.data[o + 1] = Math.round((g * alpha + image.data[o + 1] * behind) / total);
      image.data[o + 2] = Math.round((b * alpha + image.data[o + 2] * behind) / total);
      image.data[o + 3] = Math.round(total * 255);
    }
  }
}

const mix = (a, b, t) => a.map((channel, i) => Math.round(channel + (b[i] - channel) * t));

// A superellipse — the shape Windows, macOS and iOS all round their app
// tiles with. n = 4 is close enough to the squircle to look intentional and
// far enough from a rounded rectangle to not look like a mistake.
function tileShape(size) {
  const half = size / 2;
  return (x, y) => {
    const dx = Math.abs(x - half) / half;
    const dy = Math.abs(y - half) / half;
    return dx ** 4 + dy ** 4 <= 1;
  };
}

// A chevron pointing up or down: two strokes meeting at a point, kept to an
// even weight by measuring from the centre line rather than filling a
// triangle.
function chevron(cx, cy, width, thickness, direction) {
  const half = width / 2;
  return (x, y) => {
    const dx = Math.abs(x - cx);
    if (dx > half) return false;
    // The centre line of the stroke at this distance from the point.
    const centre = cy + direction * (dx - half / 2) * 0.9;
    return Math.abs(y - centre) <= thickness / 2;
  };
}

// Lay `layer` over `base`, pixel for pixel — both are the same size, and the
// layer's own alpha does the blending.
function overlay(base, layer) {
  paint(base, (x, y) => {
    const o = (Math.floor(y) * base.width + Math.floor(x)) * 4;
    const alpha = layer.data[o + 3];
    return alpha ? [layer.data[o], layer.data[o + 1], layer.data[o + 2], alpha] : false;
  });
}

// Where the colour band starts, as a fraction of the tile's height. A third
// of the tile is about five pixels at 16 px — enough colour to name the icon
// across a taskbar.
const BAND_TOP = 0.66;

// One icon at `size` pixels.
//
// The plate, the band and the arrow are shapes, so they are drawn at 4x and
// averaged down — that averaging is the anti-aliasing. The mark is artwork,
// so it is resized once, straight to the size it is drawn at. The tile's own
// coverage is kept as a mask and multiplied back in at the end, which is what
// keeps the band inside the tile's rounded corners.
function drawIcon({ app, size: tileSize, large, mark }) {
  const S = tileSize * SUPERSAMPLE;
  const band = large ? BANDS.large : BANDS.small;
  const shape = tileShape(S);
  const shade = (stops, y) => [...mix(stops[0], stops[1], y / S), 255];

  // The plate and the band in one pass, so the seam between them is a single
  // hard edge the downsample softens rather than two blended layers.
  const plate = canvas(S);
  const bandTop = S * BAND_TOP;
  paint(plate, (x, y) => {
    if (!shape(x, y)) return false;
    if (y < bandTop) return shade(app.plate, y);
    // A hairline of the plate's own dark end separates the band from it, so
    // red on Claude's clay still reads as two colours.
    if (y < bandTop + S * 0.018) return [...app.plate[1], 255];
    return shade(band.fill, y);
  });
  const icon = resizeImage(plate, tileSize, tileSize);
  // The same shape again in flat white: its alpha is how much of each
  // finished pixel is inside the tile.
  const stencil = canvas(S);
  paint(stencil, (x, y) => (shape(x, y) ? [255, 255, 255, 255] : false));
  const coverage = resizeImage(stencil, tileSize, tileSize);

  // The mark sits centred in the plate above the band, drawn large for a big
  // task and small for a small one — the same thing the band's colour says,
  // said again in size.
  const width = Math.max(1, Math.round(tileSize * (large ? 0.52 : 0.4)));
  const height = Math.max(1, Math.round(width * (mark.height / mark.width)));
  const drawn = resizeImage(mark, width, height);
  const left = Math.round((tileSize - width) / 2);
  const top = Math.round(tileSize * BAND_TOP * 0.5 - height / 2);
  paint(icon, (x, y) => {
    const mx = Math.floor(x) - left;
    const my = Math.floor(y) - top;
    if (mx < 0 || my < 0 || mx >= width || my >= height) return false;
    const alpha = drawn.data[(my * width + mx) * 4 + 3];
    return alpha ? [...app.markInk, alpha] : false;
  });

  // The arrow in the band: up for the large task, down for the small one.
  // Drawn into its own supersampled layer so its edges are as clean as the
  // plate's, then laid over the finished tile.
  const arrow = canvas(S);
  const centre = S * (BAND_TOP + (1 - BAND_TOP) / 2);
  paint(
    arrow,
    chevron(S / 2, centre, S * 0.2, S * 0.062, large ? 1 : -1),
    [255, 255, 255, 255]
  );
  overlay(icon, resizeImage(arrow, tileSize, tileSize));

  // Clip the lot back to the tile, so the band keeps the corners' curve.
  for (let i = 3; i < icon.data.length; i += 4) {
    icon.data[i] = Math.round((icon.data[i] * coverage.data[i]) / 255);
  }
  return icon;
}

// --- .ico --------------------------------------------------------------------

// An icon as the DIB an .ico entry holds: a BITMAPINFOHEADER claiming twice
// the height (the format still expects an AND mask after the pixels), then
// bottom-up BGRA rows, then that mask — all zero, because 32-bit entries are
// read through their alpha.
function dib({ width, height, data }) {
  const header = Buffer.alloc(40);
  header.writeUInt32LE(40, 0);
  header.writeInt32LE(width, 4);
  header.writeInt32LE(height * 2, 8);
  header.writeUInt16LE(1, 12);
  header.writeUInt16LE(32, 14);
  const pixels = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const from = ((height - 1 - y) * width + x) * 4;
      const to = (y * width + x) * 4;
      pixels[to] = data[from + 2];
      pixels[to + 1] = data[from + 1];
      pixels[to + 2] = data[from];
      pixels[to + 3] = data[from + 3];
    }
  }
  const maskStride = Math.ceil(width / 32) * 4;
  return Buffer.concat([header, pixels, Buffer.alloc(maskStride * height)]);
}

function ico(images) {
  const entries = images.map((image) => ({
    size: image.width,
    // 256 is the one size the format writes as 0, and the one every reader
    // expects to find PNG-compressed rather than as a raw DIB.
    body: image.width >= 256 ? encodePng(image) : dib(image)
  }));
  const header = Buffer.alloc(6);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  let offset = 6 + entries.length * 16;
  const directory = entries.map((entry) => {
    const row = Buffer.alloc(16);
    row[0] = entry.size >= 256 ? 0 : entry.size;
    row[1] = entry.size >= 256 ? 0 : entry.size;
    row.writeUInt16LE(1, 4);
    row.writeUInt16LE(32, 6);
    row.writeUInt32LE(entry.body.length, 8);
    row.writeUInt32LE(offset, 12);
    offset += entry.body.length;
    return row;
  });
  return Buffer.concat([header, ...directory, ...entries.map((entry) => entry.body)]);
}

// --- the four icons ----------------------------------------------------------

// The colour code, in bro's own meter palette (usage.js colours a figure
// green with room to spare and red when it is nearly gone). The large-task
// icon is the green one because that is the login it opens; the small-task
// icon is red for the same reason. They also differ in lightness, so the
// pair still separates for a red-green colour blind reader — and the arrow
// says it a third time, in shape.
const BANDS = {
  large: { fill: [[61, 201, 122], [34, 158, 90]] },
  small: { fill: [[240, 88, 76], [199, 48, 42]] }
};

const APPS = {
  claude: {
    name: 'claude',
    label: 'Claude',
    // The plate colour off Claude's own Windows tile (#D97757), lit from the
    // top.
    plate: [[229, 138, 110], [193, 96, 65]],
    markInk: [255, 255, 255],
    sources: () => {
      const dir = packageDir('Claude');
      return dir ? [path.join(dir, 'app', 'resources', 'Tray-Win32-Dark.ico')] : [];
    }
  },
  codex: {
    name: 'codex',
    label: 'Codex',
    plate: [[58, 58, 58], [10, 10, 10]],
    markInk: [255, 255, 255],
    sources: () => {
      const dir = packageDir('OpenAI.Codex');
      return dir ? [path.join(dir, 'assets', 'Square44x44Logo.targetsize-256_altform-unplated.png')] : [];
    }
  }
};

fs.mkdirSync(OUT_DIR, { recursive: true });
for (const app of Object.values(APPS)) {
  const loaded = loadMark(app);
  const mark = cropToInk(loaded.image);
  for (const large of [true, false]) {
    const name = `${app.name}-${large ? 'big' : 'small'}-task`;
    const png = drawIcon({ app, size: PNG_SIZE, large, mark });
    fs.writeFileSync(path.join(OUT_DIR, `${name}.png`), encodePng(png));
    fs.writeFileSync(
      path.join(OUT_DIR, `${name}.ico`),
      ico(ICO_SIZES.map((size) => drawIcon({ app, size, large, mark })))
    );
    console.log(`assets/icons/${name}.{png,ico}  ${PNG_SIZE}px + ${ICO_SIZES.join('/')}  (mark from ${loaded.from})`);
  }
}
