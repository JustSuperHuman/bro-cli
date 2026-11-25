// Gathers everything the film needs into public/ and src/data/:
//   voice-over (from a JustImagine run), measured line lengths and the scene layout, app captures put on the
//   desktop (Acrylic over the wallpaper), music-video clips, the song, Meta Muse, sounds.
//
//   node scripts/prepare.mjs [--vo <ji run output>] [--skip-clips]
//
// Without --vo it keeps the voice files already in public/vo; a line with no file gets an estimated length so the
// film can still be laid out and previewed silent. Everything that comes from outside is named in media.json.
import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const repo = resolve(root, '..', '..');
const pub = join(root, 'public');
const data = join(root, 'src', 'data');
for (const d of ['vo', 'clips', 'desk', 'song', 'sfx', 'muse', 'img']) mkdirSync(join(pub, d), { recursive: true });
mkdirSync(join(root, 'assets', 'muse'), { recursive: true });
mkdirSync(data, { recursive: true });

const arg = (name) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : null; };
const media = JSON.parse(readFileSync(join(here, 'media.json'), 'utf8'));
const script = JSON.parse(readFileSync(join(here, 'beats.json'), 'utf8'));
const newer = (src, dst) => !existsSync(dst) || statSync(src).mtimeMs > statSync(dst).mtimeMs;
const FPS = 30;

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
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
const voRun = arg('--vo');
if (voRun) {
  const text = readFileSync(voRun, 'utf8');
  const run = JSON.parse(text.slice(text.indexOf('{')));
  if (run.failed?.length) throw new Error(`voice run has ${run.failed.length} failed lines: ${JSON.stringify(run.failed).slice(0, 300)}`);
  // the server finishes lines out of order, so each file is matched to its line by the words in its name
  const used = new Set();
  for (const b of script.beats) {
    const want = slug(b.say).slice(0, 14);
    const file = run.files.find((f) => !used.has(f) && slug(f.split(/[\\/]/).pop()).includes(want));
    if (!file) continue; // a run may hold only some lines; the others keep the file already in public/vo
    used.add(file);
    copyFileSync(file, join(pub, 'vo', `${b.id}.mp3`));
  }
  console.log(`voice run: ${used.size} of ${run.files.length} files matched a line`);
}
const beats = script.beats.map((b) => {
  const file = join(pub, 'vo', `${b.id}.mp3`);
  const has = existsSync(file);
  const total = has ? seconds(file) : Math.max(1.2, b.say.length / 15.5);
  const span = has ? speech(file, total) : { begin: 0.05, end: total - 0.05 };
  return { ...b, file: has ? `vo/${b.id}.mp3` : null, seconds: +total.toFixed(3), begin: span.begin, end: span.end, show: b.show ?? b.say };
});

// ---- the layout: where each scene and line starts (all in frames) ------------------------------------------
const f = (s) => Math.round(s * FPS);
const scenes = [];
for (const b of beats) if (!script.order.includes(b.scene)) throw new Error(`${b.id}: scene ${b.scene} isn't in "order"`);
for (const id of script.order) {
  const cfg = script.scenes[id] ?? {};
  const scene = { id, from: 0, frames: 0, beats: [] };
  let cursor = cfg.lead ?? script.lead;
  beats.filter((b) => b.scene === id).forEach((b, i) => {
    if (i === 0 && b.pre !== undefined) cursor = b.pre;
    else if (i > 0) cursor += b.pre ?? script.gap;
    const dur = Math.max(b.seconds, b.min ?? 0);
    scene.beats.push({
      id: b.id, file: b.file, say: b.say, show: b.show, typed: b.typed ?? null,
      from: f(cursor), frames: f(b.seconds), slot: f(dur), begin: f(b.begin), end: f(b.end),
    });
    cursor += dur;
  });
  // a scene with no lines (the end card) is as long as its `min`
  scene.frames = Math.max(f(cursor + (cfg.tail ?? script.tail)), f(cfg.min ?? 0));
  scenes.push(scene);
}
let at = 0;
for (const s of scenes) {
  s.from = at;
  at += s.frames;
}
writeFileSync(join(data, 'timing.json'), JSON.stringify({ fps: FPS, total: at, scenes }, null, 2));
const spoken = beats.reduce((n, b) => n + b.seconds, 0);
console.log(`voice: ${beats.filter((b) => b.file).length}/${beats.length} files, ${spoken.toFixed(1)} s spoken; film ${(at / FPS).toFixed(1)} s (${at} frames)`);
for (const s of scenes) console.log(`  ${s.id.padEnd(8)} ${(s.from / FPS).toFixed(1).padStart(6)} s  ${(s.frames / FPS).toFixed(1).padStart(5)} s`);

// ---- app captures on the desktop ---------------------------------------------------------------------------
const deskShot = join(repo, 'scripts', 'desk-shot.py');
const py = process.platform === 'win32' ? 'python' : 'python3';
const put = (src, name) => {
  const dst = join(pub, 'desk', `${name}.png`);
  if (!existsSync(src)) { console.warn(`missing capture ${src}`); return; }
  if (!newer(src, dst)) return;
  const r = spawnSync(py, ['-X', 'utf8', deskShot, src, dst, '--margin', '45'], { encoding: 'utf8' });
  if (r.status !== 0) console.warn(`desk-shot failed for ${src}: ${(r.stderr || '').slice(-200)}`);
};
for (const [name, src] of Object.entries(media.captures)) {
  if (name === 'kit' && existsSync(src)) {
    // the toolkit capture's cast grid is a fixture: it is blurred, so only the Toolkit panel reads
    const masked = join(root, 'out', 'kit-masked.png');
    mkdirSync(join(root, 'out'), { recursive: true });
    if (newer(src, masked)) spawnSync(py, ['-X', 'utf8', join(here, 'kitmask.py'), src, masked], { stdio: 'inherit' });
    put(masked, name);
  } else put(src, name);
}
const listPng = (dir, re) => (existsSync(dir) ? readdirSync(dir).filter((n) => re.test(n)).sort() : []);
const tlAll = listPng(media.timelapse.dir, /^\d+\.png$/).map((n) => n.replace('.png', ''));
for (const n of tlAll) put(join(media.timelapse.dir, `${n}.png`), `tl-${n}`);
const [skipA, skipB] = media.timelapse.skip ?? [-1, -2];
const tl = tlAll.filter((n) => !(Number(n) >= skipA && Number(n) <= skipB));
const cuNames = listPng(media.closeups?.dir ?? '', /\.png$/i).map((n) => n.replace(/\.png$/i, ''));
for (const n of cuNames) put(join(media.closeups.dir, `${n}.png`), `cu-${n}`);
writeFileSync(join(data, 'assets.json'), JSON.stringify({ tl: tl.map((n) => `tl-${n}`), tlLast: `tl-${tlAll[tlAll.length - 1]}`, cu: cuNames.map((n) => `cu-${n}`), pullBox: media.pullBox, clips: Object.fromEntries(Object.entries(media.clips).map(([k, v]) => [k, { songAt: v.songAt, segments: v.segments ?? [] }])) }, null, 2));
console.log(`captures: ${Object.keys(media.captures).length} + ${tlAll.length} time-lapse frames (${tl.length} used) + ${cuNames.length} close-ups`);

// ---- the song's own words, for the lines that land on their own (cold open, alligator, end) ---------------------
{
  const words = JSON.parse(readFileSync(join(dirname(media.song.full), 'whisper-words.json'), 'utf8')).words;
  const lyrics = {};
  for (const [key, [a, b]] of Object.entries(media.lyrics)) {
    lyrics[key] = words
      .filter((w) => w.start >= a && w.start < b && w.word.trim() !== 'words')
      .map((w) => ({ text: w.word.trim(), t0: +w.start.toFixed(2), t1: +Math.min(w.end, b).toFixed(2) }));
  }
  writeFileSync(join(data, 'lyrics.json'), JSON.stringify(lyrics, null, 2));
  console.log('lyrics:', Object.entries(lyrics).map(([k, v]) => `${k} "${v.map((w) => w.text).join(' ')}"`).join(' | '));
}

// ---- Meta Muse ---------------------------------------------------------------------------------------------
for (const [pose, src] of Object.entries(media.muse)) {
  const keep = join(root, 'assets', 'muse', `${pose}.png`);
  if (existsSync(src) && newer(src, keep)) copyFileSync(src, keep);
  const dst = join(pub, 'muse', `${pose}.png`);
  if (newer(keep, dst)) spawnSync(py, ['-X', 'utf8', join(here, 'muse.py'), keep, dst], { stdio: 'inherit' });
}
if (!existsSync(join(pub, 'img', 'wall.jpg'))) spawnSync(py, ['-X', 'utf8', join(here, 'wall.py'), join(pub, 'img', 'wall.jpg')], { stdio: 'inherit' });

// ---- music-video clips, scaled to 1080p ---------------------------------------------------------------------
// Each clip names `sources` (the first that exists is used, from `ss` for `t` seconds). Until one exists the clip is
// built from `fallback`: still pictures drifting slowly, or a labelled card. The result is public/clips/<name>.mp4
// either way, and `standIn` in assets.json says which clips are still stand-ins.
const standIn = {};
const encode = (args) => { if (spawnSync(ff.ffmpeg, ['-hide_banner', '-loglevel', 'error', '-y', ...args], { stdio: 'inherit' }).status !== 0) throw new Error(`ffmpeg failed: ${args.join(' ').slice(0, 140)}`); };
const out264 = ['-an', '-c:v', 'libx264', '-crf', '17', '-preset', 'fast', '-g', '12', '-pix_fmt', 'yuv420p', '-movflags', '+faststart'];
if (!process.argv.includes('--skip-clips')) {
  for (const [name, c] of Object.entries(media.clips)) {
    const dst = join(pub, 'clips', `${name}.mp4`);
    const stamp = join(pub, 'clips', `${name}.json`);
    const src = (c.sources ?? []).find((x) => existsSync(x.src));
    const fb = c.fallback ?? {};
    // a fallback is pictures (`images`, or `parts`); a part naming a `scene` becomes that scene's take (sNN-take1.mp4) once it exists
    const takes = join(dirname(c.sources?.[0]?.src ?? ''), '..', 'clips');
    const parts = (fb.parts ?? (fb.images ?? []).map((image) => ({ image }))).map((pt) => {
      const take = pt.scene ? join(media.takes ?? takes, `s${pt.scene}-take1.mp4`) : null;
      if (take && existsSync(take)) return { clip: take, ss: pt.clipSs ?? 1, t: pt.t };
      if (pt.clip) return existsSync(pt.clip) ? pt : null;
      return existsSync(pt.image) ? pt : null;
    }).filter(Boolean);
    const key = JSON.stringify({ seg: c.segments ?? null, src: src?.src ?? null, ss: src?.ss ?? 0, t: c.t ?? 0, mtime: src ? statSync(src.src).mtimeMs : 0, fb: src ? null : fb, fbm: src ? 0 : (fb.parts ?? fb.images ?? []).map((i) => { const f = typeof i === 'string' ? i : i.image; const t = i.scene ? join(media.takes ?? '', `s${i.scene}-take1.mp4`) : null; return [existsSync(f ?? '') ? statSync(f).mtimeMs : 0, t && existsSync(t) ? statSync(t).mtimeMs : 0]; }) });
    standIn[name] = !src;
    if (existsSync(dst) && existsSync(stamp) && readFileSync(stamp, 'utf8') === key) continue;
    if (src && c.segments?.length) {
      // several stretches of one file, cut together
      const inputs = c.segments.flatMap((g) => ['-ss', String(g.ss), '-t', String(g.t), '-i', src.src]);
      const chains = c.segments.map((_, i) => `[${i}:v]scale=1920:1080:flags=lanczos,fps=24,setsar=1[v${i}]`).join(';');
      encode([...inputs, '-filter_complex', `${chains};${c.segments.map((_, i) => `[v${i}]`).join('')}concat=n=${c.segments.length}:v=1:a=0[o]`, '-map', '[o]', ...out264, dst]);
    } else if (src) {
      const args = [];
      if (src.ss) args.push('-ss', String(src.ss));
      args.push('-i', src.src);
      if (c.t) args.push('-t', String(c.t));
      encode([...args, '-vf', 'scale=1920:1080:flags=lanczos', ...out264, dst]);
    } else if (parts.length) {
      // still pictures drifting slowly, and takes where they exist, one after another
      const total = c.t ?? 4;
      const per = total / parts.length;
      const inputs = [];
      const chains = [];
      parts.forEach((pt, i) => {
        const d = pt.t ?? per;
        if (pt.clip) {
          inputs.push('-ss', String(pt.ss ?? 0), '-t', String(d), '-i', pt.clip);
          chains.push(`[${i}:v]scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,fps=24,setsar=1[v${i}]`);
        } else {
          inputs.push('-i', pt.image);
          chains.push(`[${i}:v]scale=2880:1620:force_original_aspect_ratio=increase,crop=2880:1620,zoompan=z='min(1+0.0007*on,1.09)':d=${Math.round(d * 24)}:s=1920x1080:fps=24,setsar=1[v${i}]`);
        }
      });
      encode([...inputs, '-filter_complex', `${chains.join(';')};${parts.map((_, i) => `[v${i}]`).join('')}concat=n=${parts.length}:v=1:a=0[o]`, '-map', '[o]', ...out264, dst]);
    } else if (fb.card) {
      const png = join(pub, 'clips', `${name}-card.png`);
      spawnSync(py, ['-X', 'utf8', join(here, 'card.py'), png, fb.card], { stdio: 'inherit' });
      encode(['-loop', '1', '-i', png, '-t', String(c.t ?? 4), '-r', '24', '-vf', 'scale=1920:1080', ...out264, dst]);
    } else { console.warn(`clip ${name}: nothing to build it from`); continue; }
    writeFileSync(stamp, key);
    console.log(`clip ${name}: ${seconds(dst).toFixed(2)} s${src ? ` from ${src.src}` : ' (stand-in)'}`);
  }
} else {
  for (const [name, c] of Object.entries(media.clips)) standIn[name] = !(c.sources ?? []).some((x) => existsSync(x.src));
}
const clipLengths = {};
for (const name of Object.keys(media.clips)) {
  const file = join(pub, 'clips', `${name}.mp4`);
  if (existsSync(file)) clipLengths[name] = +seconds(file).toFixed(3);
}
const assets = JSON.parse(readFileSync(join(data, 'assets.json'), 'utf8'));
assets.clipSeconds = clipLengths;
assets.standIn = standIn;
writeFileSync(join(data, 'assets.json'), JSON.stringify(assets, null, 2));
console.log('stand-in clips:', Object.entries(standIn).filter(([, v]) => v).map(([k]) => k).join(', ') || 'none');

// ---- the song, as AAC; and a bed with the voices taken out (centre cancelled above 140 Hz) ---------------------
{
  const src = media.song.full;
  const full = join(pub, 'song', 'full.m4a');
  if (existsSync(src) && newer(src, full)) encode(['-i', src, '-ar', '48000', '-c:a', 'aac', '-b:a', '256k', full]);
  const bed = join(pub, 'song', 'bed.m4a');
  if (existsSync(src) && newer(src, bed)) {
    encode(['-i', src, '-filter_complex', '[0]asplit[a][b];[a]lowpass=f=140[lo];[b]highpass=f=140,pan=stereo|c0=c0-c1|c1=c1-c0[hi];[lo][hi]amix=inputs=2:normalize=0,volume=1.6', '-ar', '48000', '-c:a', 'aac', '-b:a', '192k', bed]);
  }
}


// ---- sounds ------------------------------------------------------------------------------------------------
const pack = join(repo, 'sfx-pack.zip');
if (existsSync(pack) && !existsSync(join(pub, 'sfx', 'tick-bright.wav'))) {
  const tmp = join(root, 'out', 'sfx-unpacked');
  mkdirSync(tmp, { recursive: true });
  const tar = process.platform === 'win32' ? join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
  spawnSync(tar, ['-xf', pack, '-C', tmp], { stdio: 'inherit' });
  for (const name of ['whoosh-short.wav', 'tick-bright.wav', 'pop.wav', 'chime-success.wav', 'impact-soft.wav']) {
    const from = join(tmp, 'sfx', name);
    if (existsSync(from)) copyFileSync(from, join(pub, 'sfx', name));
  }
}
console.log('sfx:', readdirSync(join(pub, 'sfx')).join(' '));
