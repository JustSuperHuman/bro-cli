import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';
import { EXT_BY_TYPE, IMAGE_TYPES, extOf, safeName } from './justimagine-store.js';

// A repeatable cast. A character is a name, a written description, and a small
// set of reference images — saved once and picked per generation, which is the
// reference-driven half of what Higgsfield calls an avatar. (Its Soul
// Characters are a *trained* identity model; nothing here trains anything, so
// consistency comes from the references and the prompt, and is very good on
// reference-driven models rather than perfect.)
//
// The library is global — one cast, every gallery and the background service —
// and each character owns its reference images rather than pointing into a
// gallery's context folder, so deleting a gallery never guts your cast.
//
//   ~/.bro/justimagine/characters/
//     nora/
//       character.json      { name, description, cover, ts }
//       refs/<sha>.png      the reference images themselves

export const CHARACTERS_DIR = path.join(BRO_DIR, 'justimagine', 'characters');
export const MAX_REFS = 8;

const metaPath = (dir) => path.join(dir, 'character.json');
const refsDir = (dir) => path.join(dir, 'refs');
// Generated shots waiting to be kept or thrown away. They live on disk rather
// than in memory so closing the editor — or reloading the page — does not lose
// a set you just paid to generate.
const candidatesDir = (dir) => path.join(dir, 'candidates');

// Character ids double as their directory name, so the library stays legible
// when you open it in a file manager.
export function characterDir(root, id) {
  const clean = String(id ?? '').trim();
  if (!clean || clean !== path.basename(clean) || clean.startsWith('.') || clean.includes('..')) {
    throw new Error(`Invalid character id: ${id}`);
  }
  return path.join(root, clean);
}

export const idFor = (name) =>
  safeName(name)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40);

export function ensureLibrary(root = CHARACTERS_DIR) {
  fs.mkdirSync(root, { recursive: true });
  return root;
}

function readCharacter(root, id) {
  const dir = characterDir(root, id);
  let meta = {};
  try {
    meta = JSON.parse(fs.readFileSync(metaPath(dir), 'utf8'));
  } catch {
    return null;
  }
  const refs = imagesIn(refsDir(dir));
  return {
    id,
    name: meta.name || id,
    description: meta.description || '',
    // The cover is whichever reference represents the character in the picker;
    // it falls back to the first so the list is never blank.
    cover: refs.includes(meta.cover) ? meta.cover : refs[0] || '',
    refs,
    candidates: imagesIn(candidatesDir(dir)),
    ts: meta.ts || 0
  };
}

function imagesIn(dir) {
  try {
    return fs
      .readdirSync(dir, { withFileTypes: true })
      .filter((e) => e.isFile() && IMAGE_TYPES[extOf(e.name)])
      .map((e) => e.name)
      .sort();
  } catch {
    return []; // a character with nothing here yet is still a character
  }
}

export function listCharacters(root = CHARACTERS_DIR) {
  let entries = [];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return [];
  }
  return entries
    .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
    .map((e) => readCharacter(root, e.name))
    .filter(Boolean)
    .sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
}

export const getCharacter = (root, id) => readCharacter(root, id);

function writeMeta(dir, meta) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(metaPath(dir), JSON.stringify(meta, null, 2));
}

export function createCharacter(root, { name, description = '' } = {}) {
  ensureLibrary(root);
  const base = idFor(name);
  if (!base) throw new Error('Character name is empty');
  let id = base;
  for (let i = 2; fs.existsSync(path.join(root, id)); i++) id = `${base}-${i}`;
  const dir = characterDir(root, id);
  fs.mkdirSync(refsDir(dir), { recursive: true });
  writeMeta(dir, { name: safeName(name), description: String(description || '').slice(0, 2000), cover: '', ts: Date.now() });
  return readCharacter(root, id);
}

// Renaming moves the directory so the library keeps matching what you see in
// the UI; the id follows the name, and callers re-point their selection.
export function updateCharacter(root, id, { name, description, cover } = {}) {
  const current = readCharacter(root, id);
  if (!current) throw new Error(`No character "${id}"`);
  let nextId = id;
  if (name != null && safeName(name) && safeName(name) !== current.name) {
    const base = idFor(name);
    if (!base) throw new Error('Character name is empty');
    if (base !== id) {
      let candidate = base;
      for (let i = 2; fs.existsSync(path.join(root, candidate)); i++) candidate = `${base}-${i}`;
      fs.renameSync(characterDir(root, id), characterDir(root, candidate));
      nextId = candidate;
    }
  }
  const dir = characterDir(root, nextId);
  writeMeta(dir, {
    name: name != null && safeName(name) ? safeName(name) : current.name,
    description: description != null ? String(description).slice(0, 2000) : current.description,
    cover: cover != null ? path.basename(String(cover)) : current.cover,
    ts: Date.now()
  });
  return readCharacter(root, nextId);
}

export function deleteCharacter(root, id) {
  const dir = characterDir(root, id);
  if (!fs.existsSync(dir)) return false;
  fs.rmSync(dir, { recursive: true, force: true });
  return true;
}

// References are stored under the hash of their bytes, so adding the same
// picture twice is a no-op and a character never accumulates duplicates.
export function addRef(root, id, buf, type = 'image/png') {
  const character = readCharacter(root, id);
  if (!character) throw new Error(`No character "${id}"`);
  if (!buf?.length) throw new Error('Empty image.');
  const dir = characterDir(root, id);
  const hash = crypto.createHash('sha256').update(buf).digest('hex').slice(0, 16);
  const file = `${hash}.${EXT_BY_TYPE[type.toLowerCase()] || 'png'}`;
  const full = path.join(refsDir(dir), file);
  const existed = fs.existsSync(full);
  if (!existed) {
    if (character.refs.length >= MAX_REFS) throw new Error(`A character holds at most ${MAX_REFS} reference images.`);
    fs.mkdirSync(refsDir(dir), { recursive: true });
    fs.writeFileSync(full, buf);
  }
  return { file, existed, character: readCharacter(root, id) };
}

export function addRefFromDataUrl(root, id, dataUrl) {
  const m = String(dataUrl || '').match(/^data:(image\/[a-z0-9+.-]+);base64,(.+)$/is);
  if (!m) throw new Error('Expected a base64 image data URL.');
  return addRef(root, id, Buffer.from(m[2], 'base64'), m[1].toLowerCase());
}

export function deleteRef(root, id, file) {
  const dir = characterDir(root, id);
  const name = path.basename(String(file ?? ''));
  if (!name) return false;
  try {
    fs.unlinkSync(path.join(refsDir(dir), name));
  } catch {
    return false;
  }
  // The cover pointed at a file that no longer exists; readCharacter falls back
  // on its own, but persist it so the metadata does not stay stale.
  const after = readCharacter(root, id);
  if (after) writeMeta(dir, { name: after.name, description: after.description, cover: after.cover, ts: Date.now() });
  return true;
}

// ---------- generated candidates ----------

// Where a generated shot goes to be looked at before it counts. Same content
// hashing as references, so keeping one is a rename rather than a rewrite.
export function addCandidate(root, id, buf, type = 'image/png') {
  if (!readCharacter(root, id)) throw new Error(`No character "${id}"`);
  if (!buf?.length) throw new Error('Empty image.');
  const dir = candidatesDir(characterDir(root, id));
  const hash = crypto.createHash('sha256').update(buf).digest('hex').slice(0, 16);
  const file = `${hash}.${EXT_BY_TYPE[type.toLowerCase()] || 'png'}`;
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, file), buf);
  return file;
}

// Promote chosen shots into the real references, stopping at the cap rather
// than silently dropping the overflow.
export function keepCandidates(root, id, files) {
  const character = readCharacter(root, id);
  if (!character) throw new Error(`No character "${id}"`);
  const dir = characterDir(root, id);
  let room = MAX_REFS - character.refs.length;
  const kept = [];
  for (const raw of Array.isArray(files) ? files : []) {
    const name = path.basename(String(raw ?? ''));
    if (!name || !character.candidates.includes(name)) continue;
    if (room <= 0) throw new Error(`A character holds at most ${MAX_REFS} reference images — kept ${kept.length}.`);
    fs.mkdirSync(refsDir(dir), { recursive: true });
    const target = path.join(refsDir(dir), name);
    // Already a reference: the shot is a duplicate, so just drop the candidate.
    if (fs.existsSync(target)) fs.rmSync(path.join(candidatesDir(dir), name), { force: true });
    else {
      fs.renameSync(path.join(candidatesDir(dir), name), target);
      room--;
    }
    kept.push(name);
  }
  return { kept, character: readCharacter(root, id) };
}

// Throw away the ones that were not kept — all of them when no list is given.
export function clearCandidates(root, id, files) {
  const dir = candidatesDir(characterDir(root, id));
  if (!Array.isArray(files)) {
    fs.rmSync(dir, { recursive: true, force: true });
    return readCharacter(root, id);
  }
  for (const raw of files) {
    const name = path.basename(String(raw ?? ''));
    if (name) fs.rmSync(path.join(dir, name), { force: true });
  }
  return readCharacter(root, id);
}

export function resolveCandidate(root, id, file) {
  const name = path.basename(String(file ?? ''));
  if (!name) return null;
  const dir = characterDir(root, id);
  // One name, two possible homes — a kept shot keeps its filename.
  for (const full of [path.join(refsDir(dir), name), path.join(candidatesDir(dir), name)]) {
    if (fs.existsSync(full)) return full;
  }
  return null;
}

// ---------- generating a reference set ----------

// Five independent generations from one description produce five different
// people, which is useless as an identity reference. So the first shot is the
// seed and every other shot is conditioned on it — that is what makes the set
// one character seen from several angles rather than a casting call.
//
// Backgrounds are deliberately plain: these are identity references, not
// finished pictures, and scenery only gives the model something to copy later.
const VARIATION_SHOTS = [
  'a three-quarter view head-and-shoulders portrait, neutral expression',
  'a full-length standing shot, arms relaxed at their sides, facing the camera',
  'a side profile head-and-shoulders portrait, neutral expression',
  'a head-and-shoulders portrait with a warm natural smile, head turned slightly',
  'a head-and-shoulders portrait looking slightly up and away from the camera'
];

const PLAIN = 'Plain flat mid-grey studio background, soft even lighting, no props, no text, sharp focus.';

export function characterShots({ name, description } = {}, count = 5) {
  const n = Math.min(Math.max(Number(count) || 5, 1), VARIATION_SHOTS.length + 1);
  const who = [description, name ? `The character is called ${name}.` : ''].filter(Boolean).join(' ');
  return {
    // Generated only when the character has no reference to seed from.
    seed: `A reference sheet portrait of a single character: ${description || name}. Front-facing head-and-shoulders, neutral expression, looking straight at the camera. ${PLAIN}`,
    variations: VARIATION_SHOTS.slice(0, n - 1).map(
      (shot) =>
        `Using the attached reference image, draw the exact same character again as ${shot}. ` +
        `Keep their face, hair, build, colouring and clothing identical to the reference — only the angle and expression change. ` +
        `${who ? who + ' ' : ''}${PLAIN}`
    )
  };
}

// Read a character's references into the shape the upstream calls want.
export function resolveCharacters(root, ids) {
  const out = [];
  for (const raw of Array.isArray(ids) ? ids : []) {
    let character;
    try {
      character = readCharacter(root, raw);
    } catch {
      continue; // an id that isn't a valid directory name is simply unknown
    }
    if (!character) continue;
    const dir = refsDir(characterDir(root, character.id));
    const refs = [];
    for (const file of character.refs) {
      let buf;
      try {
        buf = fs.readFileSync(path.join(dir, file));
      } catch {
        continue;
      }
      const type = IMAGE_TYPES[extOf(file)] || 'image/png';
      refs.push({ file, buf, type, ext: EXT_BY_TYPE[type] || 'png', dataUrl: `data:${type};base64,${buf.toString('base64')}` });
    }
    out.push({ ...character, resolved: refs });
  }
  return out;
}

// ---------- prompt composition ----------

// The picked characters are named in the prompt and described after it, so the
// model is told both who is in the shot and that the attached references are
// them. `@Nora` typed in the prompt is normalised to plain `Nora`, which lets
// the mention read naturally in the sentence the user actually wrote.
export function composePrompt(prompt, characters = []) {
  let text = String(prompt || '');
  for (const c of characters) {
    if (!c?.name) continue;
    text = text.replace(new RegExp(`@${escapeRe(c.name)}\\b`, 'gi'), c.name);
    text = text.replace(new RegExp(`@${escapeRe(c.id)}\\b`, 'gi'), c.name);
  }
  const named = characters.filter((c) => c?.name);
  if (!named.length) return text;

  const lines = named.map((c) => `- ${c.name}${c.description ? ` — ${c.description}` : ''}`);
  const who = named.length === 1 ? 'this character' : 'these characters';
  return [
    text,
    '',
    `The reference images are ${who}. Keep their face, hair, build and clothing consistent with the references:`,
    ...lines
  ].join('\n');
}

const escapeRe = (s) => String(s).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
