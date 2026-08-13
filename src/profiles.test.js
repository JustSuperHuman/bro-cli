import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { assertProfileName, sessionRows, stageFiles } from './profiles.js';

const plain = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');

const withTempRoot = (fn) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-profiles-'));
  try {
    fn(root);
  } finally {
    if (path.dirname(path.resolve(root)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(root, { recursive: true, force: true });
  }
};

test('staging copies into the destination and cleanup leaves nothing of its own behind', () => {
  withTempRoot((root) => {
    const source = path.join(root, 'source', 'sessions', '2026', '08', '01', 'rollout.jsonl');
    const target = path.join(root, 'target', 'sessions', '2026', '08', '01', 'rollout.jsonl');
    fs.mkdirSync(path.dirname(source), { recursive: true });
    fs.writeFileSync(source, '{"type":"session_meta"}\n');

    const staged = stageFiles({ targetRoot: path.join(root, 'target'), entries: [{ source, target }] });
    expect(fs.readFileSync(target, 'utf8')).toBe('{"type":"session_meta"}\n');

    staged.cleanup();
    expect(fs.existsSync(target)).toBe(false);
    // The directories the copy had to invent go too, up to the profile root.
    expect(fs.existsSync(path.join(root, 'target', 'sessions'))).toBe(false);
    expect(fs.existsSync(source)).toBe(true);
  });
});

test('a directory alongside the transcript is staged and removed whole', () => {
  withTempRoot((root) => {
    const sourceDir = path.join(root, 'source', 'session-env', 'abc');
    const targetDir = path.join(root, 'target', 'session-env', 'abc');
    fs.mkdirSync(sourceDir, { recursive: true });
    fs.writeFileSync(path.join(sourceDir, 'env'), 'TEST=1');

    const staged = stageFiles({ targetRoot: path.join(root, 'target'), entries: [{ source: sourceDir, target: targetDir }] });
    expect(fs.readFileSync(path.join(targetDir, 'env'), 'utf8')).toBe('TEST=1');
    staged.cleanup();
    expect(fs.existsSync(targetDir)).toBe(false);
  });
});

test('staging refuses to write outside the destination profile, or over something already there', () => {
  withTempRoot((root) => {
    const source = path.join(root, 'source.jsonl');
    fs.writeFileSync(source, 'x');
    const targetRoot = path.join(root, 'target');

    expect(() => stageFiles({ targetRoot, entries: [{ source, target: path.join(root, 'escaped.jsonl') }] }))
      .toThrow(/outside the destination profile/);

    const occupied = path.join(targetRoot, 'sessions', 'rollout.jsonl');
    fs.mkdirSync(path.dirname(occupied), { recursive: true });
    fs.writeFileSync(occupied, 'theirs');
    expect(() => stageFiles({ targetRoot, entries: [{ source, target: occupied }] }))
      .toThrow(/already has session artifact/);
    expect(fs.readFileSync(occupied, 'utf8')).toBe('theirs');
  });
});

test('a failed stage rolls back the copies that had already landed', () => {
  withTempRoot((root) => {
    const first = path.join(root, 'first.jsonl');
    const second = path.join(root, 'second.jsonl');
    fs.writeFileSync(first, 'one');
    fs.writeFileSync(second, 'two');
    const targetRoot = path.join(root, 'target');
    const landed = path.join(targetRoot, 'sessions', 'first.jsonl');

    expect(() => stageFiles({
      targetRoot,
      entries: [
        { source: first, target: landed },
        { source: second, target: path.join(root, 'escaped.jsonl') }
      ]
    })).toThrow(/outside the destination profile/);
    expect(fs.existsSync(landed)).toBe(false);
  });
});

test('session rows group by project and tag each row with the login that owns it', () => {
  const rows = sessionRows(
    [
      { id: 'a1', title: 'fix the bridge', cwd: 'F:\\bro-cli', branch: 'main', account: null, mtime: Date.now(), current: true },
      { id: 'b2', title: 'ship the picker', cwd: 'J:\\justgains', branch: 'dev', account: 'work', mtime: Date.now(), current: false }
    ],
    (s) => ({ kind: 'session', id: s.id })
  );

  expect(rows.map((r) => (r.divider ? r.label : 'session'))).toEqual([
    'resume · this project',
    'session',
    'resume · other projects',
    'session'
  ]);
  expect(plain(rows[1].label)).toContain('fix the bridge');
  expect(plain(rows[1].label)).toContain('main · local');
  expect(plain(rows[3].label)).toContain('J:\\justgains · dev · work');
  // Searchable by id and full path even when the row had to shorten them.
  expect(rows[3].filterText).toContain('b2');
  expect(rows[3].filterText).toContain('J:\\justgains');
  expect(rows[3].value).toEqual({ kind: 'session', id: 'b2' });
});

test('a list with no history at all contributes no rows, not an empty heading', () => {
  expect(sessionRows([], (s) => s.id)).toEqual([]);
});

test('profile names may not escape the profile root', () => {
  expect(assertProfileName('work')).toBe('work');
  expect(assertProfileName(' work-2.1 ')).toBe('work-2.1');
  for (const bad of ['', '..', '.', 'a/b', 'a\\b', 'a:b', '~']) {
    expect(() => assertProfileName(bad)).toThrow(/Invalid profile name/);
  }
});
