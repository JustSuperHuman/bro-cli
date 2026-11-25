// Writes the JustImagine voice spec for the narration: one item per beat, in order.
//   node scripts/vo-spec.mjs <out.json>
// Then: node ~/.claude/skills/generate-voice/scripts/ji.mjs plan <out.json> / run <out.json>
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const script = JSON.parse(readFileSync(join(here, 'beats.json'), 'utf8'));
const out = process.argv[2];
if (!out) throw new Error('Give the path to write the spec to.');
const only = process.argv.slice(3);

const spec = {
  defaults: { kind: 'voice', folder: script.folder, model: script.model, voice: script.voice, stability: 0.5, similarity: 0.8, style: 0.2, speed: 1.05 },
  items: script.beats.filter((b) => !only.length || only.includes(b.id)).map((b) => ({ prompt: b.say })),
};
writeFileSync(out, JSON.stringify(spec, null, 2));
console.log(`${spec.items.length} lines, ${spec.items.reduce((n, i) => n + i.prompt.length, 0)} characters -> ${out}`);
