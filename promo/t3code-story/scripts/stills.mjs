// Renders single frames of the story for checking by eye, bundling once.
//   node scripts/stills.mjs --every 60            one frame every 60
//   node scripts/stills.mjs --frames 120,480,900  these frames
//   node scripts/stills.mjs --beats               the middle and the end of every narrated line
//   node scripts/stills.mjs --table               only print where each scene and line starts
// Frames land in out/stills/f<frame>.jpg.
import { bundle } from '@remotion/bundler';
import { renderStill, selectComposition } from '@remotion/renderer';
import { mkdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const arg = (name) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : null; };
const has = (name) => process.argv.includes(name);

// The same sums as src/timeline.ts, to name frames by scene and line.
const FPS = 30, LEAD = 0.55, GAP = 0.45, TAIL = 0.95;
const fr = (s) => Math.round(s * FPS);
const timing = JSON.parse(readFileSync(join(root, 'src', 'data', 'timing.json'), 'utf8'));
const scenes = [];
for (const b of timing.beats) {
  let scene = scenes[scenes.length - 1];
  if (!scene || scene.id !== b.scene) scenes.push((scene = { id: b.scene, from: 0, frames: fr(LEAD), list: [] }));
  const frames = fr(b.seconds);
  const slot = Math.max(frames + fr(GAP), fr(b.min ?? 0));
  scene.list.push({ id: b.id, from: scene.frames, frames, slot });
  scene.frames += slot;
}
let at = 0;
for (const scene of scenes) { scene.frames += fr(TAIL - GAP); scene.from = at; at += scene.frames; }
const total = at;

console.log(`total ${total} frames, ${(total / FPS).toFixed(1)} s`);
for (const s of scenes) console.log(`${s.id.padEnd(7)} ${String(s.from).padStart(5)}  ${(s.from / FPS).toFixed(1).padStart(6)} s  ` + s.list.map((b) => `${b.id}@${s.from + b.from}`).join(' '));
if (has('--table')) process.exit(0);

let frames = [];
if (arg('--frames')) frames = arg('--frames').split(',').map(Number);
else if (has('--beats')) for (const s of scenes) for (const b of s.list) frames.push(s.from + b.from + Math.round(b.frames * 0.55), s.from + b.from + b.slot - 4);
else { const every = Number(arg('--every') || 90); for (let f = Math.round(every / 2); f < total; f += every) frames.push(f); }
frames = [...new Set(frames.filter((f) => f >= 0 && f < total))].sort((a, b) => a - b);

const out = join(root, 'out', 'stills');
mkdirSync(out, { recursive: true });
const serveUrl = await bundle({ entryPoint: join(root, 'src', 'index.ts') });
const composition = await selectComposition({ serveUrl, id: 'Story' });
if (composition.durationInFrames !== total) console.warn(`the composition has ${composition.durationInFrames} frames; this script worked out ${total}`);
const scale = Number(arg('--scale') || 1);
for (const frame of frames) {
  await renderStill({ composition, serveUrl, frame, output: join(out, `f${String(frame).padStart(5, '0')}.jpg`), imageFormat: 'jpeg', jpegQuality: 88, scale });
  process.stdout.write(`${frame} `);
}
console.log(`\n${frames.length} frames in ${out}`);
