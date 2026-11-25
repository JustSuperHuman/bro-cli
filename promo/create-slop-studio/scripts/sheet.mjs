// A contact sheet of a rendered film: one frame every N seconds, with its time.
//   node scripts/sheet.mjs out/rough.mp4 out/rough-sheet.jpg 2.5
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [input, output, every = '2.5'] = process.argv.slice(2);
const winget = join(process.env.LOCALAPPDATA || '', 'Microsoft', 'WinGet', 'Packages');
let ff = 'ffmpeg';
if (existsSync(winget)) for (const pkg of readdirSync(winget).filter((n) => n.startsWith('Gyan.FFmpeg'))) for (const b of readdirSync(join(winget, pkg)).filter((n) => n.startsWith('ffmpeg'))) ff = join(winget, pkg, b, 'bin', 'ffmpeg.exe');
const tmp = join(root, 'out', 'sheet-frames');
rmSync(tmp, { recursive: true, force: true });
mkdirSync(tmp, { recursive: true });
const r = spawnSync(ff, ['-hide_banner', '-loglevel', 'error', '-y', '-i', input, '-vf', `fps=1/${every},scale=480:-1`, join(tmp, 'f%03d.jpg')], { stdio: 'inherit' });
if (r.status !== 0) throw new Error('ffmpeg failed');
const py = spawnSync('python', ['-X', 'utf8', '-c', `
import glob, sys
from PIL import Image, ImageDraw
fs = sorted(glob.glob(r'${tmp.replace(/\\/g, '/')}/f*.jpg'))
cols = 6
ims = [Image.open(f) for f in fs]
w, h = ims[0].size
rows = (len(ims) + cols - 1) // cols
s = Image.new('RGB', (cols * w, rows * h), (0, 0, 0))
for i, im in enumerate(ims):
    s.paste(im, ((i % cols) * w, (i // cols) * h))
    t = i * ${every}
    ImageDraw.Draw(s).text(((i % cols) * w + 6, (i // cols) * h + 4), '%d:%04.1f' % (t // 60, t % 60), fill=(255, 220, 120))
s.save(r'${resolve(root, output).replace(/\\/g, '/')}', quality=88)
print(len(ims), 'frames')
`], { stdio: 'inherit' });
if (py.status !== 0) throw new Error('sheet failed');
