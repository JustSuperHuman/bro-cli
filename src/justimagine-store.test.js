import { afterEach, beforeEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  appendHistory,
  contextDir,
  createFolder,
  deleteContext,
  deleteFolder,
  deleteItem,
  ensureRoot,
  imageSize,
  kindOf,
  listContext,
  listFolders,
  listItems,
  migrateLegacy,
  modelTimings,
  moveItems,
  readHistory,
  renameFolder,
  resolveFile,
  resolveFolder,
  resolveRefs,
  safeName,
  saveContext,
  saveThumb,
  thumbPath,
  tooSmallRefs
} from './justimagine-store.js';

let root;

beforeEach(() => {
  root = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-store-'));
  ensureRoot(root);
});
afterEach(() => fs.rmSync(root, { recursive: true, force: true }));

const write = (rel, file, bytes = 'x') => {
  const dir = path.join(root, ...(rel ? rel.split('/') : []));
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, file), bytes);
};

const PNG = 'data:image/png;base64,' + Buffer.from('fake-png').toString('base64');
const JPEG = 'data:image/jpeg;base64,' + Buffer.from('fake-jpeg').toString('base64');

// ---------- image dimensions ----------

// Real encoded bytes, not hand-built headers — a parser that only agrees with
// my own fixture proves nothing. These are the smallest valid files of each
// format, with dimensions that are asymmetric so width/height can't be swapped
// without the test noticing.
const REAL = {
  // 2x1 PNG
  png: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==', 'base64'),
  // 3x2 JPEG
  jpeg: Buffer.from(
    '/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAACAAMBAREA/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/9oACAEBAAA/APn+v//Z',
    'base64'
  ),
  // 4x3 GIF
  gif: Buffer.from('R0lGODdhBAADAIAAAP///////ywAAAAABAADAAACA4SPCgA7', 'base64'),
  // 5x4 WebP (lossy VP8)
  webp: Buffer.from('UklGRkYAAABXRUJQVlA4WAoAAAAQAAAABAAAAwAAQUxQSAwAAAABBxAR/Q9ERP8DAABWUDggGAAAADABAJ0BKgUABAACADQlpAADcAD+/gbQAA==', 'base64')
};

test('dimensions are read from the header of every format we accept', () => {
  expect(imageSize(REAL.png)).toEqual({ width: 2, height: 1 });
  expect(imageSize(REAL.jpeg)).toEqual({ width: 3, height: 2 });
  expect(imageSize(REAL.gif)).toEqual({ width: 4, height: 3 });
  expect(imageSize(REAL.webp)).toEqual({ width: 5, height: 4 });
});

test('anything unreadable reports no size rather than a wrong one', () => {
  expect(imageSize(Buffer.from('not an image at all, just text'))).toBe(null);
  expect(imageSize(Buffer.alloc(4))).toBe(null);
  expect(imageSize(Buffer.alloc(0))).toBe(null);
  expect(imageSize(null)).toBe(null);
  // a truncated PNG still has its IHDR, which is the point of reading the header
  expect(imageSize(REAL.png.subarray(0, 24))).toEqual({ width: 2, height: 1 });
});

test('undersized references are singled out by name', () => {
  const refs = [
    { file: 'tiny.png', buf: REAL.png }, // 2x1
    { file: 'unknown.bin', buf: Buffer.from('nope') },
    { file: 'big.png', buf: REAL.png }
  ];
  expect(tooSmallRefs(refs, 300).map((r) => r.file)).toEqual(['tiny.png', 'big.png']);
  expect(tooSmallRefs(refs, 300)[0]).toEqual({ file: 'tiny.png', width: 2, height: 1 });
  // a file we cannot measure is never accused
  expect(tooSmallRefs(refs, 300).some((r) => r.file === 'unknown.bin')).toBe(false);
  // below the bar is what matters, not the format
  expect(tooSmallRefs(refs, 1)).toEqual([]);
  expect(tooSmallRefs([])).toEqual([]);
  expect(tooSmallRefs(undefined)).toEqual([]);
});

// ---------- path safety ----------

test('folder paths are confined to the root', () => {
  expect(resolveFolder(root, '').rel).toBe('');
  expect(resolveFolder(root, 'a/b').rel).toBe('a/b');
  expect(resolveFolder(root, '/a//b/').rel).toBe('a/b');
  expect(resolveFolder(root, 'a\\b').rel).toBe('a/b');
  for (const bad of ['..', 'a/../..', '../escape', '.context', 'a/.thumbs', 'C:/windows']) {
    expect(() => resolveFolder(root, bad)).toThrow();
  }
});

test('file names are flattened to a basename so they cannot walk out', () => {
  expect(resolveFile(root, 'a', '../../etc/passwd').name).toBe('passwd');
  expect(resolveFile(root, 'a', 'clip.mp4').full).toBe(path.join(root, 'a', 'clip.mp4'));
  expect(() => resolveFile(root, 'a', '')).toThrow();
});

test('safeName strips separators and trims to one usable segment', () => {
  expect(safeName('  My Shots / v2  ')).toBe('My Shots v2');
  expect(safeName('...')).toBe('');
  expect(safeName('a:b*c?')).toBe('a b c');
});

test('kindOf classifies by extension', () => {
  expect(kindOf('a.png')).toBe('image');
  expect(kindOf('a.MP4')).toBe('video');
  expect(kindOf('a.webm')).toBe('video');
  expect(kindOf('history.jsonl')).toBe('');
});

// ---------- folders ----------

test('the folder tree counts media and nests, hiding reserved dirs', () => {
  write('', 'a.png');
  write('shoots', 'b.png');
  write('shoots/night', 'c.mp4');
  write('shoots/night', 'notes.txt');
  fs.mkdirSync(path.join(root, '.thumbs'), { recursive: true });

  const tree = listFolders(root);
  expect(tree.path).toBe('');
  expect(tree.items).toBe(1);
  expect(tree.total).toBe(3);
  expect(tree.children.map((c) => c.path)).toEqual(['shoots']);
  const shoots = tree.children[0];
  expect(shoots.items).toBe(1);
  expect(shoots.total).toBe(2);
  expect(shoots.children[0].path).toBe('shoots/night');
  expect(shoots.children[0].items).toBe(1);
});

test('create, rename and nest folders', () => {
  expect(createFolder(root, '', 'Shoots')).toBe('Shoots');
  expect(createFolder(root, 'Shoots', 'Night')).toBe('Shoots/Night');
  expect(() => createFolder(root, '', 'Shoots')).toThrow(/already exists/);
  expect(() => createFolder(root, '', '   ')).toThrow(/empty/);
  expect(renameFolder(root, 'Shoots/Night', 'Dusk')).toBe('Shoots/Dusk');
  expect(fs.existsSync(path.join(root, 'Shoots', 'Dusk'))).toBe(true);
  expect(() => renameFolder(root, '', 'nope')).toThrow(/top-level/);
});

test('deleting a folder deletes every generation inside it', () => {
  write('trip', 'a.png');
  write('trip/day2', 'b.png');
  write('trip/day2', 'c.mp4');
  write('keep', 'd.png');

  expect(deleteFolder(root, 'trip')).toBe(3);
  expect(fs.existsSync(path.join(root, 'trip'))).toBe(false);
  expect(fs.existsSync(path.join(root, 'keep', 'd.png'))).toBe(true);
});

test('deleting a folder takes the thumbnails of everything inside it too', () => {
  write('trip', 'a.png');
  write('trip/day2', 'b.mp4');
  write('keep', 'd.png');
  saveThumb(root, 'trip', 'a.png', JPEG);
  saveThumb(root, 'trip/day2', 'b.mp4', JPEG);
  saveThumb(root, 'keep', 'd.png', JPEG);

  expect(deleteFolder(root, 'trip')).toBe(2);
  expect(fs.existsSync(thumbPath(root, 'trip', 'a.png'))).toBe(false);
  expect(fs.existsSync(thumbPath(root, 'trip/day2', 'b.mp4'))).toBe(false);
  // an untouched folder keeps its cache
  expect(fs.existsSync(thumbPath(root, 'keep', 'd.png'))).toBe(true);
  expect(fs.readdirSync(path.join(root, '.thumbs')).length).toBe(1);
});

test('deleting the top level empties it but keeps the root and its reserved dirs', () => {
  write('', 'a.png');
  write('sub', 'b.png');
  saveContext(root, PNG);

  expect(deleteFolder(root, '')).toBe(2);
  expect(fs.existsSync(root)).toBe(true);
  expect(fs.existsSync(path.join(root, 'sub'))).toBe(false);
  expect(listContext(root).length).toBe(1);
});

// ---------- items + history ----------

test('listItems merges history onto the files actually on disk, newest first', () => {
  const dir = path.join(root, 'set');
  write('set', 'old.png');
  write('set', 'new.mp4');
  appendHistory(dir, { file: 'old.png', kind: 'image', prompt: 'a cat', model: 'm1', ts: 1000 });
  appendHistory(dir, { file: 'new.mp4', kind: 'video', prompt: 'a dog', model: 'm2', ts: 2000 });
  appendHistory(dir, { file: 'gone.png', kind: 'image', prompt: 'deleted', ts: 3000 });

  const items = listItems(root, 'set');
  expect(items.map((i) => i.file)).toEqual(['new.mp4', 'old.png']);
  expect(items[0].kind).toBe('video');
  expect(items[0].prompt).toBe('a dog');
  expect(items[1].folder).toBe('set');
  expect(items[0].bytes).toBeGreaterThan(0);
  // the stale entry is compacted away rather than accumulating forever
  expect([...readHistory(dir).keys()].sort()).toEqual(['new.mp4', 'old.png']);
});

test('a file dropped in by hand still appears, dated from the filesystem', () => {
  write('set', 'manual.png');
  const items = listItems(root, 'set');
  expect(items.length).toBe(1);
  expect(items[0].kind).toBe('image');
  expect(items[0].ts).toBeGreaterThan(0);
});

test('a torn final history line does not lose the rest', () => {
  const dir = path.join(root, 'set');
  write('set', 'a.png');
  fs.appendFileSync(path.join(dir, 'history.jsonl'), JSON.stringify({ file: 'a.png', prompt: 'kept' }) + '\n{"file":"b.p');
  expect(listItems(root, 'set')[0].prompt).toBe('kept');
});

test('deleteItem removes the file, its history row and its thumbnail', () => {
  const dir = path.join(root, 'set');
  write('set', 'a.png');
  appendHistory(dir, { file: 'a.png', prompt: 'x' });
  saveThumb(root, 'set', 'a.png', JPEG);
  expect(fs.existsSync(thumbPath(root, 'set', 'a.png'))).toBe(true);

  expect(deleteItem(root, 'set', 'a.png')).toBe(true);
  expect(fs.existsSync(path.join(dir, 'a.png'))).toBe(false);
  expect(readHistory(dir).size).toBe(0);
  expect(fs.existsSync(thumbPath(root, 'set', 'a.png'))).toBe(false);
});

// ---------- moving ----------

test('moving carries metadata and the cached thumbnail across', () => {
  write('from', 'a.png');
  appendHistory(path.join(root, 'from'), { file: 'a.png', prompt: 'travels', model: 'm' });
  saveThumb(root, 'from', 'a.png', JPEG);

  expect(moveItems(root, 'from', 'to', ['a.png'])).toEqual({ moved: 1 });
  expect(fs.existsSync(path.join(root, 'to', 'a.png'))).toBe(true);
  expect(readHistory(path.join(root, 'from')).size).toBe(0);
  expect(readHistory(path.join(root, 'to')).get('a.png').prompt).toBe('travels');
  expect(fs.existsSync(thumbPath(root, 'to', 'a.png'))).toBe(true);
  expect(fs.existsSync(thumbPath(root, 'from', 'a.png'))).toBe(false);
});

test('a name collision at the destination is renamed, not overwritten', () => {
  write('from', 'a.png', 'source');
  write('to', 'a.png', 'existing');
  expect(moveItems(root, 'from', 'to', ['a.png']).moved).toBe(1);
  expect(fs.readFileSync(path.join(root, 'to', 'a.png'), 'utf8')).toBe('existing');
  expect(fs.readFileSync(path.join(root, 'to', 'a-1.png'), 'utf8')).toBe('source');
});

test('moving into the same folder, or moving a non-media file, is a no-op', () => {
  write('from', 'a.png');
  write('from', 'notes.txt');
  expect(moveItems(root, 'from', 'from', ['a.png'])).toEqual({ moved: 0 });
  expect(moveItems(root, 'from', 'to', ['notes.txt', 'missing.png'])).toEqual({ moved: 0 });
});

// ---------- context + thumbs ----------

test('context images dedupe by content hash', () => {
  const first = saveContext(root, PNG);
  const second = saveContext(root, PNG);
  expect(first.existed).toBe(false);
  expect(second.existed).toBe(true);
  expect(second.file).toBe(first.file);
  expect(listContext(root).map((c) => c.file)).toEqual([first.file]);
  expect(() => saveContext(root, 'not-a-data-url')).toThrow();

  const refs = resolveRefs(root, [first.file, 'missing.png']);
  expect(refs.length).toBe(1);
  expect(refs[0].dataUrl.startsWith('data:image/png;base64,')).toBe(true);
  expect(refs[0].buf.toString()).toBe('fake-png');

  expect(deleteContext(root, first.file)).toBe(true);
  expect(listContext(root).length).toBe(0);
});

test('resolveRefs caps how many references are loaded', () => {
  const files = [];
  for (let i = 0; i < 10; i++) files.push(saveContext(root, 'data:image/png;base64,' + Buffer.from('p' + i).toString('base64')).file);
  expect(resolveRefs(root, files).length).toBe(8);
});

test('thumbnails only accept JPEG and are keyed by folder + name', () => {
  write('set', 'a.png');
  expect(() => saveThumb(root, 'set', 'a.png', PNG)).toThrow(/JPEG/);
  saveThumb(root, 'set', 'a.png', JPEG);
  expect(fs.existsSync(thumbPath(root, 'set', 'a.png'))).toBe(true);
  expect(thumbPath(root, 'set', 'a.png')).not.toBe(thumbPath(root, 'other', 'a.png'));
});

// ---------- migration ----------

test('an existing bro image-gen folder is folded into the new gallery root', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-legacy-'));
  const legacyOut = path.join(base, '.bro', 'image-gen');
  const legacyCtx = path.join(base, '.bro', 'context');
  fs.mkdirSync(legacyOut, { recursive: true });
  fs.mkdirSync(legacyCtx, { recursive: true });
  fs.writeFileSync(path.join(legacyOut, 'old.png'), 'x');
  fs.writeFileSync(path.join(legacyOut, 'history.jsonl'), JSON.stringify({ file: 'old.png', prompt: 'legacy' }) + '\n');
  fs.writeFileSync(path.join(legacyCtx, 'ref.png'), 'y');
  const target = path.join(base, '.bro', 'justimagine');

  const moved = migrateLegacy(target, legacyOut, legacyCtx);
  expect(moved).toEqual({ images: 1, context: 1 });
  expect(listItems(target, '')[0].prompt).toBe('legacy');
  expect(listContext(target).map((c) => c.file)).toEqual(['ref.png']);
  expect(fs.existsSync(legacyOut)).toBe(false);

  // Running again must not disturb the migrated gallery.
  expect(migrateLegacy(target, legacyOut, legacyCtx)).toEqual({ images: 0, context: 0 });
  expect(listItems(target, '').length).toBe(1);
  fs.rmSync(base, { recursive: true, force: true });
});

test('migration leaves an already-populated root alone', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-legacy2-'));
  const legacyOut = path.join(base, 'image-gen');
  fs.mkdirSync(legacyOut, { recursive: true });
  fs.writeFileSync(path.join(legacyOut, 'old.png'), 'x');
  const target = path.join(base, 'justimagine');
  ensureRoot(target);
  fs.writeFileSync(path.join(target, 'mine.png'), 'z');

  expect(migrateLegacy(target, legacyOut, path.join(base, 'context')).images).toBe(0);
  expect(listItems(target, '').map((i) => i.file)).toEqual(['mine.png']);
  expect(fs.existsSync(path.join(legacyOut, 'old.png'))).toBe(true);
  fs.rmSync(base, { recursive: true, force: true });
});

test('ensureRoot creates the reserved directories', () => {
  const fresh = path.join(root, 'nested', 'deep');
  ensureRoot(fresh);
  expect(fs.existsSync(contextDir(fresh))).toBe(true);
  expect(fs.existsSync(path.join(fresh, '.thumbs'))).toBe(true);
});

test('model timings are the median generation time per model across every folder', () => {
  const sub = createFolder(root, '', 'portraits');
  appendHistory(root, { file: 'a.png', kind: 'image', model: 'gemini', ms: 9000, ts: 1 });
  appendHistory(root, { file: 'b.png', kind: 'image', model: 'gemini', ms: 15000, ts: 2 });
  appendHistory(path.join(root, sub), { file: 'c.png', kind: 'image', model: 'gemini', ms: 30000, ts: 3 });
  appendHistory(path.join(root, sub), { file: 'd.mp4', kind: 'video', model: 'veo', ms: 90000, ts: 4 });
  appendHistory(root, { file: 'e.png', kind: 'image', model: 'veo', ts: 5 }); // no duration recorded
  appendHistory(root, { file: 'f.png', kind: 'image', ms: 5, ts: 6 }); // no model
  expect(modelTimings(root)).toEqual({ gemini: { median: 15000, n: 3 }, veo: { median: 90000, n: 1 } });
  expect(modelTimings(path.join(root, 'nowhere'))).toEqual({});
});
