// Brings a render's sound to a steady loudness (-16 LUFS, peaks under -1.5 dB) without touching the picture.
//   node scripts/level.mjs <in.mp4> <out.mp4>
// Two passes of ffmpeg's loudnorm: the first measures, the second applies what was measured.
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const [input, output] = process.argv.slice(2);
if (!input || !output) throw new Error('Give the file to read and the file to write.');

function ffmpeg() {
  const dirs = [process.env.FFMPEG_BIN, ''].filter((d) => d !== undefined && d !== null);
  const winget = join(process.env.LOCALAPPDATA || '', 'Microsoft', 'WinGet', 'Packages');
  if (existsSync(winget)) for (const pkg of readdirSync(winget).filter((n) => n.startsWith('Gyan.FFmpeg'))) for (const build of readdirSync(join(winget, pkg))) dirs.push(join(winget, pkg, build, 'bin'));
  for (const d of dirs) {
    const exe = d ? join(d, 'ffmpeg' + (process.platform === 'win32' ? '.exe' : '')) : 'ffmpeg';
    if (spawnSync(exe, ['-version'], { stdio: 'ignore' }).status === 0) return exe;
  }
  throw new Error('ffmpeg not found. Set FFMPEG_BIN to the folder that holds it.');
}
const ff = ffmpeg();
const TARGET = 'I=-16:TP=-1.5:LRA=11';

const first = spawnSync(ff, ['-hide_banner', '-i', input, '-vn', '-af', `loudnorm=${TARGET}:print_format=json`, '-f', 'null', '-'], { encoding: 'utf8' }).stderr || '';
const json = /\{[^{}]*"input_i"[^{}]*\}/.exec(first)?.[0];
if (!json) throw new Error(`ffmpeg didn't report the loudness of ${input}:\n${first.slice(-600)}`);
const m = JSON.parse(json);

const filter = `loudnorm=${TARGET}:measured_I=${m.input_i}:measured_TP=${m.input_tp}:measured_LRA=${m.input_lra}:measured_thresh=${m.input_thresh}:offset=${m.target_offset}:linear=true`;
const second = spawnSync(ff, ['-hide_banner', '-loglevel', 'error', '-y', '-i', input, '-c:v', 'copy', '-af', filter, '-ar', '48000', '-c:a', 'aac', '-b:a', '192k', '-movflags', '+faststart', output], { stdio: 'inherit' });
if (second.status !== 0) throw new Error('ffmpeg failed to write the levelled file.');
console.log(`${input}: ${m.input_i} LUFS, peak ${m.input_tp} dB -> ${output} at about -16 LUFS`);
