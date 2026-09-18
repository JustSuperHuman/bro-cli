import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { scanSkills } from './skills.js';

function fixture(context) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-skills-'));
  context.after(() => {
    assert.equal(path.dirname(path.resolve(root)), fs.realpathSync(os.tmpdir()));
    fs.rmSync(root, { recursive: true, force: true });
  });
  return root;
}

function skill(root, tool, name, content = '# Skill') {
  const directory = path.join(root, tool, 'skills', name);
  fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(directory, 'SKILL.md'), content);
  return directory;
}

test('scan is read-only, discovers both tools, and includes canonical-only skills', (context) => {
  const root = fixture(context);
  skill(root, '.claude', 'review');
  skill(root, '.codex', 'test');
  skill(root, '.agents', 'design');
  const before = fs.readdirSync(root, { recursive: true });
  const result = scanSkills(root);
  assert.deepEqual(result.candidates.map((entry) => [entry.name, entry.status]), [['design', 'ready'], ['review', 'ready'], ['test', 'ready']]);
  assert.deepEqual(fs.readdirSync(root, { recursive: true }), before);
});

test('identical duplicates are ready, different content and occupied paths are blocked', (context) => {
  const root = fixture(context);
  skill(root, '.claude', 'same');
  skill(root, '.codex', 'same');
  skill(root, '.claude', 'different', 'first');
  skill(root, '.codex', 'different', 'second');
  skill(root, '.claude', 'occupied');
  fs.writeFileSync(path.join(root, '.codex', 'skills', 'occupied'), 'keep');
  const candidates = scanSkills(root).candidates;
  assert.equal(candidates.find((entry) => entry.name === 'same').status, 'ready');
  assert.equal(candidates.find((entry) => entry.name === 'different').status, 'blocked');
  assert.equal(candidates.find((entry) => entry.name === 'occupied').status, 'blocked');
});

test('empty roots stay empty and case variants are discovered', (context) => {
  const root = fixture(context);
  assert.equal(scanSkills(root).candidates.length, 0);
  assert.deepEqual(fs.readdirSync(root), []);
  skill(root, '.Claude', 'review');
  assert.equal(scanSkills(root).candidates[0].status, 'ready');
  assert.equal(path.basename(path.dirname(scanSkills(root).directories.claude)), '.Claude');
});

test('tool-managed system skills and unrelated files are never candidates for moving', (context) => {
  const root = fixture(context);
  skill(root, '.codex', '.system');
  fs.writeFileSync(path.join(root, '.codex', 'skills', 'README.md'), 'keep');
  const result = scanSkills(root);
  assert.equal(result.ignored.length, 1);
  assert.equal(result.candidates[0].status, 'blocked');
});
