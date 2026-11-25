// Gathers everything the story needs into public/ and src/data/:
//   voice-over (from a JustImagine run), measured beat lengths, screenshots, sounds, a music bed,
//   and the numbers read from the real ~/.bro/v2-perf.log.
//
//   node scripts/prepare.mjs [--vo <ji run output>] [--perf <v2-perf.log>]
//
// Without --vo it keeps the voice files already in public/vo; a beat with no file gets an estimated length
// so the story can still be laid out and previewed silent.
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const repo = resolve(root, '..', '..');
const pub = join(root, 'public');
const data = join(root, 'src', 'data');
for (const d of ['vo', 'img', 'sfx', 'music']) mkdirSync(join(pub, d), { recursive: true });
mkdirSync(data, { recursive: true });

const arg = (name) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : null; };

// ---- ffmpeg ------------------------------------------------------------------------------------------------
function findFf() {
  const names = ['ffmpeg', 'ffprobe'];
  const dirs = [process.env.FFMPEG_BIN, ''].filter((d) => d !== undefined && d !== null);
  const winget = join(process.env.LOCALAPPDATA || '', 'Microsoft', 'WinGet', 'Packages');
  if (existsSync(winget)) {
    for (const pkg of readdirSync(winget).filter((n) => n.startsWith('Gyan.FFmpeg'))) {
      for (const build of readdirSync(join(winget, pkg))) dirs.push(join(winget, pkg, build, 'bin'));
    }
  }
  for (const d of dirs) {
    const [ffmpeg, ffprobe] = names.map((n) => (d ? join(d, n + (process.platform === 'win32' ? '.exe' : '')) : n));
    if (spawnSync(ffprobe, ['-version'], { stdio: 'ignore' }).status === 0) return { ffmpeg, ffprobe };
  }
  throw new Error('ffmpeg/ffprobe not found. Set FFMPEG_BIN to the folder that holds them.');
}
const ff = findFf();

const seconds = (file) => parseFloat(execFileSync(ff.ffprobe, ['-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', file], { encoding: 'utf8' }).trim());

/** Where speech starts and ends in a voice file (seconds). */
function speech(file, total) {
  const out = spawnSync(ff.ffmpeg, ['-hide_banner', '-i', file, '-af', 'silencedetect=noise=-42dB:d=0.08', '-f', 'null', '-'], { encoding: 'utf8' }).stderr || '';
  const starts = [...out.matchAll(/silence_start: ([\d.-]+)/g)].map((m) => Math.max(0, parseFloat(m[1])));
  const ends = [...out.matchAll(/silence_end: ([\d.-]+)/g)].map((m) => parseFloat(m[1]));
  let begin = 0;
  let end = total;
  if (starts.length && starts[0] <= 0.02 && ends.length) begin = ends[0];
  const last = starts[starts.length - 1];
  if (last !== undefined && (ends.length < starts.length || ends[ends.length - 1] >= total - 0.02) && last > begin) end = last;
  return { begin: +begin.toFixed(3), end: +end.toFixed(3) };
}

// ---- voice-over --------------------------------------------------------------------------------------------
const script = JSON.parse(readFileSync(join(here, 'beats.json'), 'utf8'));
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
const voRun = arg('--vo');
if (voRun) {
  const text = readFileSync(voRun, 'utf8');
  const run = JSON.parse(text.slice(text.indexOf('{')));
  if (run.failed?.length) throw new Error(`voice run has ${run.failed.length} failed lines: ${JSON.stringify(run.failed).slice(0, 300)}`);
  if (run.files.length !== script.beats.length) throw new Error(`voice run has ${run.files.length} files for ${script.beats.length} beats`);
  script.beats.forEach((b, i) => {
    const file = run.files[i];
    const want = slug(b.say).slice(0, 12);
    if (!slug(file).includes(want)) throw new Error(`${b.id}: ${file} doesn't look like "${b.say.slice(0, 30)}…"`);
    copyFileSync(file, join(pub, 'vo', `${b.id}.mp3`));
  });
}
const beats = script.beats.map((b) => {
  const file = join(pub, 'vo', `${b.id}.mp3`);
  const has = existsSync(file);
  const total = has ? seconds(file) : Math.max(1.6, b.say.length / 15.5);
  const span = has ? speech(file, total) : { begin: 0.05, end: total - 0.05 };
  return { id: b.id, scene: b.scene, file: has ? `vo/${b.id}.mp3` : null, seconds: +total.toFixed(3), begin: span.begin, end: span.end, min: b.min ?? 0, say: b.say, show: b.show ?? b.say };
});
writeFileSync(join(data, 'timing.json'), JSON.stringify({ beats }, null, 2));
const spoken = beats.reduce((n, b) => n + b.seconds, 0);
console.log(`voice: ${beats.filter((b) => b.file).length}/${beats.length} files, ${spoken.toFixed(1)} s spoken`);

// ---- pictures ----------------------------------------------------------------------------------------------
const shots = join(repo, 'reports', 't3code-deep-dive', 'img');
for (const name of ['computers-sessions.png', 'computers-closed.png', 'computers-unreachable.png', 'computers-signed-out.png']) copyFileSync(join(shots, name), join(pub, 'img', name));
copyFileSync(join(repo, 'crates', 'bro-tui', 'assets', 'bro.png'), join(pub, 'img', 'bro.png'));

// ---- sounds ------------------------------------------------------------------------------------------------
const pack = join(repo, 'sfx-pack.zip');
if (existsSync(pack)) {
  const tmp = join(root, 'out', 'sfx-unpacked');
  mkdirSync(tmp, { recursive: true });
  // Windows' own tar reads zip files; Git's tar (first on PATH in Git Bash) takes "F:" for a remote host
  const tar = process.platform === 'win32' ? join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
  spawnSync(tar, ['-xf', pack, '-C', tmp], { stdio: 'inherit' });
  for (const name of ['whoosh-short.wav', 'tick-bright.wav', 'pop.wav', 'chime-success.wav', 'impact-soft.wav']) {
    const from = join(tmp, 'sfx', name);
    if (existsSync(from)) copyFileSync(from, join(pub, 'sfx', name));
  }
}
const peaks = {};
for (const name of readdirSync(join(pub, 'sfx'))) {
  const out = spawnSync(ff.ffmpeg, ['-hide_banner', '-i', join(pub, 'sfx', name), '-af', 'volumedetect', '-f', 'null', '-'], { encoding: 'utf8' }).stderr || '';
  peaks[name] = { max: parseFloat(/max_volume: ([-\d.]+) dB/.exec(out)?.[1] ?? '0'), mean: parseFloat(/mean_volume: ([-\d.]+) dB/.exec(out)?.[1] ?? '0'), seconds: +seconds(join(pub, 'sfx', name)).toFixed(2) };
}
const voLevels = beats.filter((b) => b.file).slice(0, 6).map((b) => {
  const out = spawnSync(ff.ffmpeg, ['-hide_banner', '-i', join(pub, b.file), '-af', 'volumedetect', '-f', 'null', '-'], { encoding: 'utf8' }).stderr || '';
  return { max: parseFloat(/max_volume: ([-\d.]+) dB/.exec(out)?.[1] ?? '0'), mean: parseFloat(/mean_volume: ([-\d.]+) dB/.exec(out)?.[1] ?? '0') };
});
writeFileSync(join(data, 'levels.json'), JSON.stringify({ sfx: peaks, vo: voLevels }, null, 2));
console.log('sfx:', Object.entries(peaks).map(([n, p]) => `${n} peak ${p.max} dB`).join(' · '));
console.log('vo levels (first 6):', voLevels.map((l) => `${l.mean}/${l.max}`).join(' '));

// ---- music bed: a quiet pad, 32 s, loops without a seam ----------------------------------------------------
{
  const rate = 44100;
  const chordSeconds = 8;
  const chords = [
    [110.0, 164.81, 220.0, 246.94, 261.63, 392.0], // Am9
    [87.31, 130.81, 174.61, 220.0, 261.63, 329.63], // Fmaj7
    [130.81, 196.0, 246.94, 329.63, 392.0], // Cmaj7
    [98.0, 146.83, 196.0, 246.94, 329.63, 440.0], // G6/9
  ];
  const total = chordSeconds * chords.length;
  const n = rate * total;
  const left = new Float32Array(n);
  const right = new Float32Array(n);
  const fade = 3.0; // seconds each chord overlaps the next
  chords.forEach((chord, c) => {
    const start = c * chordSeconds;
    chord.forEach((freq, v) => {
      const level = (v === 0 ? 0.9 : 0.55) / (1 + v * 0.22);
      for (const [detune, pan] of [[-0.0016, 0.25], [0.0016, 0.75]]) {
        const w = 2 * Math.PI * freq * (1 + detune);
        const lfo = 2 * Math.PI * (0.07 + 0.013 * v);
        const phase = v * 1.7 + (detune > 0 ? 0.9 : 0);
        // each chord runs half a fade past both ends, so its fade-out lies exactly under the next one's fade-in
        for (let k = Math.round((-fade / 2) * rate); k < (chordSeconds + fade / 2) * rate; k++) {
          const t = k / rate;
          const u = t + fade / 2;
          // raised cosine: the two chords' gains sum to one across the overlap
          const gain = u < fade ? 0.5 - 0.5 * Math.cos((Math.PI * u) / fade) : u > chordSeconds ? 0.5 + 0.5 * Math.cos((Math.PI * (u - chordSeconds)) / fade) : 1;
          const at = (((start * rate + k) % n) + n) % n; // wraps: the last chord's tail lands on the first one's head
          const tt = start + t;
          const s = (Math.sin(w * tt + phase) + 0.16 * Math.sin(2 * w * tt + phase * 1.3) + 0.05 * Math.sin(3 * w * tt)) * level * gain * (0.86 + 0.14 * Math.sin(lfo * tt + phase));
          left[at] += s * (1 - pan);
          right[at] += s * pan;
        }
      }
    });
  });
  let peak = 0;
  for (let i = 0; i < n; i++) peak = Math.max(peak, Math.abs(left[i]), Math.abs(right[i]));
  const scale = 0.25 / peak; // -12 dBFS
  const buf = Buffer.alloc(44 + n * 4);
  buf.write('RIFF', 0); buf.writeUInt32LE(36 + n * 4, 4); buf.write('WAVEfmt ', 8); buf.writeUInt32LE(16, 16); buf.writeUInt16LE(1, 20); buf.writeUInt16LE(2, 22);
  buf.writeUInt32LE(rate, 24); buf.writeUInt32LE(rate * 4, 28); buf.writeUInt16LE(4, 32); buf.writeUInt16LE(16, 34); buf.write('data', 36); buf.writeUInt32LE(n * 4, 40);
  for (let i = 0; i < n; i++) {
    buf.writeInt16LE(Math.round(left[i] * scale * 32767), 44 + i * 4);
    buf.writeInt16LE(Math.round(right[i] * scale * 32767), 46 + i * 4);
  }
  writeFileSync(join(pub, 'music', 'pad.wav'), buf);
  console.log(`music bed: ${total} s loop`);
}

// ---- the numbers -------------------------------------------------------------------------------------------
// The first set is the reading of the log taken on 2026-11-10 at 11:40, before 2.1.49 was installed.
const before = {
  from: '9 Nov 21:11', to: '10 Nov 11:40', hours: 14.5, total: 2405,
  parts: [
    { key: 'gpu', label: 'Waiting on graphics', count: 1844, p50: 67, p90: 115, max: 9338 },
    { key: 'render', label: 'bro drawing', count: 366, p50: 69, p90: 134, max: 1624 },
    { key: 'unattributed', label: 'Unattributed', count: 188, p50: 64, p90: 182, max: 9895 },
    { key: 'housekeeping', label: 'Housekeeping', count: 5, p50: 67, p90: 119, max: 119 },
    { key: 'events', label: 'Events', count: 2, p50: 211, p90: 211, max: 211 },
  ],
  measured: { fifo: { frames: 1522, worst: 55.0, step: 'acquire', stepMs: 53.2 }, mailbox: { frames: 1506, worst: 71.8, step: 'present', stepMs: 70.6 }, normal: 1.2 },
};
// The second is read now: passes logged by 2.1.49, which split the graphics part into its three steps.
const perfLog = arg('--perf') || join(process.env.USERPROFILE || process.env.HOME || '', '.bro', 'v2-perf.log');
const since = { found: false };
if (existsSync(perfLog)) {
  const num = (re, s) => { const m = re.exec(s); return m ? parseFloat(m[1]) : null; };
  const rows = [];
  let surface = null;
  for (const line of readFileSync(perfLog, 'utf8').split(/\r?\n/)) {
    const drawn = /window draws on (.+)$/.exec(line);
    if (drawn) surface = drawn[1].trim();
    if (!/slow pass/.test(line) || !/acquire/.test(line)) continue;
    rows.push({
      at: line.slice(0, 23), line: line.slice(24), total: num(/slow pass ([\d.]+) ms/, line), render: num(/render ([\d.]+) ms/, line), events: num(/events ([\d.]+) ms/, line),
      house: num(/housekeeping ([\d.]+) ms/, line), gpu: num(/gpu ([\d.]+) ms/, line), acquire: num(/acquire ([\d.]+) ms/, line), draw: num(/draw ([\d.]+) ms/, line), present: num(/present ([\d.]+) ms/, line),
    });
  }
  const gpu = rows.filter((r) => r.gpu >= Math.max(r.render, r.events, r.house) && r.gpu + r.render + r.events + r.house >= r.total * 0.5);
  const by = { acquire: 0, draw: 0, present: 0 };
  for (const r of gpu) by[Object.entries({ acquire: r.acquire, draw: r.draw, present: r.present }).sort((a, b) => b[1] - a[1])[0][0]]++;
  if (rows.length) {
    const t0 = Date.parse(rows[0].at.replace(' ', 'T'));
    const t1 = Date.parse(rows[rows.length - 1].at.replace(' ', 'T'));
    // the line shown is a middling one of the commonest kind, so it matches the count beside it
    const top = (r) => Object.entries({ acquire: r.acquire, draw: r.draw, present: r.present }).sort((a, b) => b[1] - a[1])[0][0];
    const commonest = Object.entries(by).sort((a, b) => b[1] - a[1])[0][0];
    const alike = gpu.filter((r) => top(r) === commonest).sort((a, b) => b.gpu - a.gpu);
    const sample = alike[Math.floor(alike.length / 2)] ?? rows[rows.length - 1];
    Object.assign(since, {
      found: true, surface, from: rows[0].at, to: rows[rows.length - 1].at, minutes: Math.round((t1 - t0) / 60000), passes: rows.length, graphics: gpu.length, by,
      held: rows.filter((r) => r.gpu >= 400).length, worst: Math.max(...rows.map((r) => r.gpu)),
      sample: { total: sample.total, gpu: sample.gpu, acquire: sample.acquire, draw: sample.draw, present: sample.present },
    });
  }
}
writeFileSync(join(data, 'perf.json'), JSON.stringify({ before, since }, null, 2));
console.log('perf since 2.1.49:', JSON.stringify(since));
