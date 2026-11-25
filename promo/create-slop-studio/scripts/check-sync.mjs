// Checks a rendered file against the timeline: does each spoken line start where the story expects it?
//   node scripts/check-sync.mjs out/bro-t3code-story.mp4
// The quiet between two lines (music bed only) is found with ffmpeg's silencedetect; where each quiet ends, a
// line starts. Those starts are compared with the frames src/timeline.ts gives the lines.
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const file = process.argv[2];
if (!file) throw new Error('Give the rendered file.');

function ffmpeg() {
  const dirs = [process.env.FFMPEG_BIN, ''].filter((d) => d !== undefined && d !== null);
  const winget = join(process.env.LOCALAPPDATA || '', 'Microsoft', 'WinGet', 'Packages');
  if (existsSync(winget)) for (const pkg of readdirSync(winget).filter((n) => n.startsWith('Gyan.FFmpeg'))) for (const build of readdirSync(join(winget, pkg))) dirs.push(join(winget, pkg, build, 'bin'));
  for (const d of dirs) {
    const exe = d ? join(d, 'ffmpeg' + (process.platform === 'win32' ? '.exe' : '')) : 'ffmpeg';
    if (spawnSync(exe, ['-version'], { stdio: 'ignore' }).status === 0) return exe;
  }
  throw new Error('ffmpeg not found. Set FFMPEG_BIN.');
}

const FPS = 30, LEAD = 0.55, GAP = 0.45, TAIL = 0.95;
const fr = (s) => Math.round(s * FPS);
const timing = JSON.parse(readFileSync(join(root, 'src', 'data', 'timing.json'), 'utf8'));
const scenes = [];
for (const b of timing.beats) {
  let scene = scenes[scenes.length - 1];
  if (!scene || scene.id !== b.scene) scenes.push((scene = { id: b.scene, from: 0, frames: fr(LEAD), list: [] }));
  const frames = fr(b.seconds);
  scene.list.push({ id: b.id, from: scene.frames, begin: b.begin, end: b.end });
  scene.frames += Math.max(frames + fr(GAP), fr(b.min ?? 0));
}
let at = 0;
for (const scene of scenes) { scene.frames += fr(TAIL - GAP); scene.from = at; at += scene.frames; }
const expected = scenes.flatMap((s) => s.list.map((b) => ({ id: b.id, start: (s.from + b.from) / FPS + b.begin, end: (s.from + b.from) / FPS + b.end })));

const noise = process.argv[3] || '-25dB';
const out = spawnSync(ffmpeg(), ['-hide_banner', '-i', file, '-vn', '-af', `silencedetect=noise=${noise}:d=0.25`, '-f', 'null', '-'], { encoding: 'utf8' }).stderr || '';
const starts = [...out.matchAll(/silence_end: ([\d.]+)/g)].map((m) => parseFloat(m[1]));
const ends = [...out.matchAll(/silence_start: ([\d.]+)/g)].map((m) => parseFloat(m[1]));

let worst = 0;
let missed = 0;
for (const line of expected) {
  const near = (list, t) => list.reduce((best, x) => (Math.abs(x - t) < Math.abs(best - t) ? x : best), Infinity);
  const s = near(starts, line.start);
  const e = near(ends, line.end);
  const ds = s - line.start;
  const de = e - line.end;
  const ok = Math.abs(ds) < 0.25;
  if (!ok) missed++;
  else worst = Math.max(worst, Math.abs(ds));
  console.log(`${line.id}  expected ${line.start.toFixed(2).padStart(7)} s  heard ${Number.isFinite(s) ? s.toFixed(2).padStart(7) : '      ?'} s  ${ok ? `${ds >= 0 ? '+' : ''}${ds.toFixed(2)}` : 'no start found nearby'}   end ${Math.abs(de) < 0.4 ? `${de >= 0 ? '+' : ''}${de.toFixed(2)}` : '(not separated)'}`);
}
console.log(`\n${expected.length - missed}/${expected.length} lines start within a quarter second of where the timeline puts them; the largest difference among them is ${worst.toFixed(2)} s.`);
process.exit(missed ? 1 : 0);
