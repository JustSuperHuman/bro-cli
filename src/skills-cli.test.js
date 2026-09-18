import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { consolidateSkills } from './skills-cli.js';

function fixture(context) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-skills-cli-'));
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

test('consolidation moves one identical skill once, links both tools, and enables undo', (context) => {
  const root = fixture(context);
  const claudePath = skill(root, '.claude', 'shared');
  const codexPath = skill(root, '.codex', 'shared');
  const claudeContent = fs.readFileSync(path.join(claudePath, 'SKILL.md'), 'utf8');
  const codexContent = fs.readFileSync(path.join(codexPath, 'SKILL.md'), 'utf8');

  const result = consolidateSkills(root, ['shared'], { rootName: 'shared.history' });
  assert.equal(result.operations.length, 1);

  const canonical = path.join(root, '.agents', 'skills', 'shared');
  assert.equal(fs.readFileSync(path.join(canonical, 'SKILL.md'), 'utf8'), '# Skill');
  assert.equal(fs.realpathSync(path.join(root, '.claude', 'skills', 'shared')), fs.realpathSync(canonical));
  assert.equal(fs.realpathSync(path.join(root, '.codex', 'skills', 'shared')), fs.realpathSync(canonical));

  const undo = consolidateSkills(root, [], { undo: true, rootName: 'shared.history' });
  assert.deepEqual(undo.undone, ['shared']);
  assert.equal(fs.readFileSync(path.join(claudePath, 'SKILL.md'), 'utf8'), claudeContent);
  assert.equal(fs.readFileSync(path.join(codexPath, 'SKILL.md'), 'utf8'), codexContent);
});

test('a missing canonical skill cannot be linked or silently undone', (context) => {
  const root = fixture(context);
  skill(root, '.claude', 'gone');
  fs.mkdirSync(path.join(root, '.agents', 'skills'), { recursive: true });
  fs.writeFileSync(path.join(root, '.agents', 'skills', 'gone'), 'occupied');
  assert.throws(() => consolidateSkills(root, ['gone']), /Not a skill directory/);
  assert.throws(() => consolidateSkills(root, [], { undo: true }), /No skills history found/);
});

test('consolidation can be requested for an explicit ready skill', (context) => {
  const root = fixture(context);
  skill(root, '.claude', 'review');
  const result = consolidateSkills(root, ['review'], { rootName: 'history' });
  assert.deepEqual(result.operations.map((operation) => operation.name), ['review']);
});
