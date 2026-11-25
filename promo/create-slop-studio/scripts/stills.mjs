// Renders single frames of the film for checking by eye, bundling once.
//   node scripts/stills.mjs --table                 only print where each scene and line starts
//   node scripts/stills.mjs --every 75              one frame every 75 (2.5 s)
//   node scripts/stills.mjs --frames 120,480,900    these frames
//   node scripts/stills.mjs --beats                 the middle of every narrated line
//   node scripts/stills.mjs --scene tools           six frames across one scene
// Frames land in out/stills/f<frame>.jpg.
import { bundle } from '@remotion/bundler';
import { renderStill, selectComposition } from '@remotion/renderer';
import { mkdirSync, readFileSync, readdirSync, unlinkSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const arg = (name) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : null; };
const has = (name) => process.argv.includes(name);

const timing = JSON.parse(readFileSync(join(root, 'src', 'data', 'timing.json'), 'utf8'));
const FPS = timing.fps;
console.log(`total ${timing.total} frames, ${(timing.total / FPS).toFixed(1)} s`);
for (const s of timing.scenes) console.log(`${s.id.padEnd(8)} ${String(s.from).padStart(5)}  ${(s.from / FPS).toFixed(1).padStart(6)} s  ` + s.beats.map((b) => `${b.id}@${s.from + b.from}`).join(' '));
if (has('--table')) process.exit(0);

let frames = [];
if (arg('--frames')) frames = arg('--frames').split(',').map(Number);
else if (arg('--scene')) {
  const s = timing.scenes.find((x) => x.id === arg('--scene'));
  for (let i = 1; i <= 6; i++) frames.push(s.from + Math.round((s.frames * i) / 7));
} else if (has('--beats')) for (const s of timing.scenes) for (const b of s.beats) frames.push(s.from + b.from + Math.round(b.frames * 0.5));
else { const every = Number(arg('--every') || 75); for (let f = Math.round(every / 2); f < timing.total; f += every) frames.push(f); }
frames = [...new Set(frames.filter((f) => f >= 0 && f < timing.total))].sort((a, b) => a - b);

const out = join(root, 'out', 'stills');
mkdirSync(out, { recursive: true });
if (has('--clean')) for (const n of readdirSync(out)) unlinkSync(join(out, n));
const serveUrl = await bundle({ entryPoint: join(root, 'src', 'index.ts') });
const composition = await selectComposition({ serveUrl, id: 'Film' });
const scale = Number(arg('--scale') || 1);
for (const frame of frames) {
  await renderStill({ composition, serveUrl, frame, output: join(out, `f${String(frame).padStart(5, '0')}.jpg`), imageFormat: 'jpeg', jpegQuality: 88, scale });
  process.stdout.write(`${frame} `);
}
console.log(`\n${frames.length} frames in ${out}`);
