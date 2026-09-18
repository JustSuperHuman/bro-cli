// Small images in the terminal — just enough to draw an app icon in a menu.
//
// Three protocols cover the terminals that can show pictures at all:
//
//   sixel  — Windows Terminal 1.22+, xterm, foot, WezTerm, Konsole, mlterm.
//            The image is drawn from pixels we encode here, so it needs the
//            PNG decoded, scaled to the cell size and reduced to a palette.
//   iterm  — iTerm2 and WezTerm's OSC 1337: the PNG goes over as-is, sized
//            in cells by the terminal.
//   kitty  — kitty and Ghostty: the PNG is uploaded once, then placed with
//            Unicode placeholder characters that behave exactly like text,
//            so repaints, erases and scrolling need no special care.
//
// Nothing here knows about menus; icons.js decides what to draw where.

import zlib from 'node:zlib';

// --- PNG ---------------------------------------------------------------------

// Decode a non-interlaced 8-bit PNG (greyscale, RGB, palette, grey+alpha or
// RGBA) into RGBA pixels. That covers every icon an app ships; anything else
// throws, and the caller falls back to text.
export function decodePng(buffer) {
  if (buffer.readUInt32BE(0) !== 0x89504e47) throw new Error('not a PNG');
  let pos = 8;
  let header = null;
  let palette = null;
  let transparency = null;
  const compressed = [];
  while (pos < buffer.length) {
    const length = buffer.readUInt32BE(pos);
    const type = buffer.toString('latin1', pos + 4, pos + 8);
    const data = buffer.subarray(pos + 8, pos + 8 + length);
    if (type === 'IHDR') {
      header = { width: data.readUInt32BE(0), height: data.readUInt32BE(4), depth: data[8], color: data[9], interlace: data[12] };
    } else if (type === 'PLTE') palette = data;
    else if (type === 'tRNS') transparency = data;
    else if (type === 'IDAT') compressed.push(data);
    else if (type === 'IEND') break;
    pos += 12 + length;
  }
  const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[header?.color];
  if (!header || !channels || header.depth !== 8 || header.interlace) throw new Error('unsupported PNG');

  const { width, height, color } = header;
  const stride = width * channels;
  const raw = zlib.inflateSync(Buffer.concat(compressed));
  const pixels = Buffer.alloc(width * height * 4);
  let previous = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const start = y * (stride + 1);
    const filter = raw[start];
    const line = Buffer.from(raw.subarray(start + 1, start + 1 + stride));
    for (let i = 0; i < stride; i++) {
      const left = i >= channels ? line[i - channels] : 0;
      const up = previous[i];
      const corner = i >= channels ? previous[i - channels] : 0;
      let predicted = 0;
      if (filter === 1) predicted = left;
      else if (filter === 2) predicted = up;
      else if (filter === 3) predicted = (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - corner;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - corner);
        predicted = pa <= pb && pa <= pc ? left : pb <= pc ? up : corner;
      }
      line[i] = (line[i] + predicted) & 255;
    }
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      if (color === 6) line.copy(pixels, o, x * 4, x * 4 + 4);
      else if (color === 2) {
        line.copy(pixels, o, x * 3, x * 3 + 3);
        pixels[o + 3] = 255;
      } else if (color === 3) {
        const index = line[x];
        palette.copy(pixels, o, index * 3, index * 3 + 3);
        pixels[o + 3] = transparency && index < transparency.length ? transparency[index] : 255;
      } else {
        const grey = line[x * channels];
        pixels[o] = pixels[o + 1] = pixels[o + 2] = grey;
        pixels[o + 3] = color === 4 ? line[x * 2 + 1] : 255;
      }
    }
    previous = line;
  }
  return { width, height, data: pixels };
}

// Encode RGBA pixels as a PNG.
export function encodePng({ width, height, data }) {
  const table = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (bytes) => {
    let c = 0xffffffff;
    for (const byte of bytes) c = table[(c ^ byte) & 255] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, body) => {
    const typed = Buffer.concat([Buffer.from(type, 'latin1'), body]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(body.length);
    const check = Buffer.alloc(4);
    check.writeUInt32BE(crc(typed));
    return Buffer.concat([length, typed, check]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6;
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) data.copy(raw, y * (width * 4 + 1) + 1, y * width * 4, (y + 1) * width * 4);
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk('IHDR', header),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

// Scale RGBA pixels to width × height by averaging the area each target pixel
// covers, in premultiplied alpha so transparent edges don't darken.
export function resizeImage(image, width, height) {
  const out = Buffer.alloc(width * height * 4);
  const sx = image.width / width;
  const sy = image.height / height;
  for (let ty = 0; ty < height; ty++) {
    for (let tx = 0; tx < width; tx++) {
      let r = 0, g = 0, b = 0, a = 0, area = 0;
      const x0 = tx * sx, x1 = x0 + sx, y0 = ty * sy, y1 = y0 + sy;
      for (let y = Math.floor(y0); y < Math.ceil(y1); y++) {
        const wy = Math.min(y + 1, y1) - Math.max(y, y0);
        for (let x = Math.floor(x0); x < Math.ceil(x1); x++) {
          const w = (Math.min(x + 1, x1) - Math.max(x, x0)) * wy;
          const o = (y * image.width + x) * 4;
          const alpha = image.data[o + 3] / 255;
          r += image.data[o] * alpha * w;
          g += image.data[o + 1] * alpha * w;
          b += image.data[o + 2] * alpha * w;
          a += alpha * w;
          area += w;
        }
      }
      const o = (ty * width + tx) * 4;
      if (a > 0) {
        out[o] = Math.round(r / a);
        out[o + 1] = Math.round(g / a);
        out[o + 2] = Math.round(b / a);
      }
      out[o + 3] = Math.round((a / area) * 255);
    }
  }
  return { width, height, data: out };
}

// --- sixel -------------------------------------------------------------------

// Up to `maxColors` palette entries, chosen by how many visible pixels fall in
// each 4-bit-per-channel bucket — icons are a few flat colours plus their
// anti-aliasing, which this keeps. Each bucket is represented by its most
// common exact colour, so a flat colour comes through exactly rather than
// averaged with the edge pixels near it.
function quantize(image, maxColors, alphaCutoff) {
  const buckets = new Map();
  for (let i = 0; i < image.data.length; i += 4) {
    if (image.data[i + 3] < alphaCutoff) continue;
    const [r, g, b] = [image.data[i], image.data[i + 1], image.data[i + 2]];
    const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4);
    const bucket = buckets.get(key) || { count: 0, colors: new Map() };
    const exact = (r << 16) | (g << 8) | b;
    bucket.count++;
    bucket.colors.set(exact, (bucket.colors.get(exact) || 0) + 1);
    buckets.set(key, bucket);
  }
  return [...buckets.values()]
    .sort((a, b) => b.count - a.count)
    .slice(0, maxColors)
    .map((bucket) => {
      const [exact] = [...bucket.colors].reduce((best, entry) => (entry[1] > best[1] ? entry : best));
      return [(exact >> 16) & 255, (exact >> 8) & 255, exact & 255];
    });
}

// A DECSIXEL image of RGBA pixels. Pixels under `alphaCutoff` are left
// unpainted (P2=1), so the terminal's own background shows through them; the
// raster attributes ask for square pixels.
export function sixelImage(image, { maxColors = 64, alphaCutoff = 128 } = {}) {
  const palette = quantize(image, maxColors, alphaCutoff);
  const { width, height, data } = image;
  const nearest = (o) => {
    let best = 0;
    let bestDistance = Infinity;
    for (let i = 0; i < palette.length; i++) {
      const [r, g, b] = palette[i];
      const distance = (data[o] - r) ** 2 + (data[o + 1] - g) ** 2 + (data[o + 2] - b) ** 2;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = i;
      }
    }
    return best;
  };
  const indices = new Int16Array(width * height).fill(-1);
  for (let p = 0; p < width * height; p++) if (data[p * 4 + 3] >= alphaCutoff) indices[p] = nearest(p * 4);

  let out = `\x1bP0;1;0q"1;1;${width};${height}`;
  palette.forEach(([r, g, b], i) => {
    out += `#${i};2;${Math.round((r / 255) * 100)};${Math.round((g / 255) * 100)};${Math.round((b / 255) * 100)}`;
  });
  for (let band = 0; band < height; band += 6) {
    const rows = [];
    for (let color = 0; color < palette.length; color++) {
      let line = '';
      let used = false;
      let run = '';
      let runLength = 0;
      const flush = () => {
        if (!runLength) return;
        line += runLength > 3 ? `!${runLength}${run}` : run.repeat(runLength);
        runLength = 0;
      };
      for (let x = 0; x < width; x++) {
        let bits = 0;
        for (let bit = 0; bit < 6 && band + bit < height; bit++) {
          if (indices[(band + bit) * width + x] === color) bits |= 1 << bit;
        }
        if (bits) used = true;
        const char = String.fromCharCode(63 + bits);
        if (char === run) runLength++;
        else {
          flush();
          run = char;
          runLength = 1;
        }
      }
      // Trailing empty columns needn't be sent.
      if (run !== '?') flush();
      if (used) rows.push(`#${color}${line}`);
    }
    out += rows.join('$') + (band + 6 < height ? '-' : '');
  }
  return `${out}\x1b\\`;
}

// --- iTerm2 ------------------------------------------------------------------

export function itermImage(png, { columns, rows }) {
  return `\x1b]1337;File=inline=1;size=${png.length};width=${columns};height=${rows};preserveAspectRatio=1:${png.toString('base64')}\x07`;
}

// --- kitty -------------------------------------------------------------------

// Upload a PNG under `id` and give it a virtual placement `columns` × `rows`
// cells big, drawn wherever the placeholder characters for it are printed.
// Quiet (q=2): the terminal sends no reply that could land in the input.
export function kittyUpload(png, { id, columns, rows }) {
  const payload = png.toString('base64');
  const chunks = payload.match(/.{1,4096}/g) || [''];
  return chunks
    .map((chunk, i) => {
      const more = i < chunks.length - 1 ? 1 : 0;
      const keys = i === 0 ? `a=T,U=1,f=100,i=${id},c=${columns},r=${rows},q=2,m=${more}` : `m=${more}`;
      return `\x1b_G${keys};${chunk}\x1b\\`;
    })
    .join('');
}

// The row/column numbers kitty reads from combining marks after each
// placeholder (the first entries of its diacritics table).
const KITTY_DIACRITICS = [0x0305, 0x030d, 0x030e, 0x0310, 0x0312, 0x033d, 0x033e, 0x033f];

// Text that shows row 0 of image `id` across `columns` cells. The image id is
// carried in the foreground colour, so it must stay below 256 here.
export function kittyPlaceholder({ id, columns }) {
  let cells = '';
  for (let column = 0; column < columns; column++) {
    cells += `\u{10EEEE}${String.fromCodePoint(KITTY_DIACRITICS[0], KITTY_DIACRITICS[column])}`;
  }
  return `\x1b[38;5;${id}m${cells}\x1b[39m`;
}

// --- what the terminal can do ------------------------------------------------

// An OSC colour reply's rgb:RRRR/GGGG/BBBB, each 1–4 hex digits, as 0–255.
function oscColor(reply) {
  const match = /rgb:([0-9a-f]{1,4})\/([0-9a-f]{1,4})\/([0-9a-f]{1,4})/i.exec(reply || '');
  if (!match) return null;
  return match.slice(1).map((hex) => Math.round((parseInt(hex, 16) / (16 ** hex.length - 1)) * 255));
}

// Ask the terminal for its background colour, its cell size in pixels and its
// primary device attributes (whose "4" means sixel). Every VT-style terminal
// answers DA1, so the wait normally ends as soon as that reply arrives; the
// timeout only guards terminals that say nothing. Resolves
// { sixel, cell: { width, height } | null, background: [r, g, b] | null }.
export function probeTerminal({ input, output, timeoutMs = 400 }) {
  return new Promise((resolve) => {
    let received = '';
    const wasRaw = Boolean(input.isRaw);
    const finish = (attributes) => {
      clearTimeout(timer);
      input.removeListener('data', onData);
      if (!wasRaw) input.setRawMode(false);
      input.pause();
      const cell = /\x1b\[6;(\d+);(\d+)t/.exec(received);
      resolve({
        sixel: Boolean(attributes?.includes('4')),
        cell: cell ? { height: Number(cell[1]), width: Number(cell[2]) } : null,
        background: oscColor(/\x1b\]11;([^\x07\x1b]*)/.exec(received)?.[1])
      });
    };
    const onData = (chunk) => {
      received += typeof chunk === 'string' ? chunk : chunk.toString('latin1');
      const reply = /\x1b\[\?([\d;]*)c/.exec(received);
      if (reply) finish(reply[1].split(';'));
    };
    const timer = setTimeout(() => finish(null), timeoutMs);
    try {
      input.setRawMode(true);
      input.resume();
      input.on('data', onData);
      // DA1 last: a terminal that doesn't know the other two ignores them,
      // and DA1 — answered by everyone — marks the end of the replies.
      output.write('\x1b]11;?\x1b\\\x1b[16t\x1b[c');
    } catch {
      finish(null);
    }
  });
}
