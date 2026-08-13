import { beforeEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  codexHomeFor,
  codexProfileLabel,
  codexSessionEntries,
  createCodexProfile,
  importCodexProfile,
  listCodexProfiles,
  removeCodexProfile
} from './codex-profiles.js';

// The profile root and the machine's codex home are pointed at temporary
// directories — no test may touch a real login. Both are resolved per call, so
// each test states them again: test files share one process, and a root left
// over from another file would reach its fixtures.
const ROOT = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-codex-profiles-'));
const HOME = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-codex-home-'));

beforeEach(() => {
  process.env.BRO_CODEX_PROFILES_DIR = ROOT;
  process.env.CODEX_HOME = HOME;
});

const plain = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');
const authFile = (dir, plan = 'pro') => {
  // A token is a JWT only in shape here; the plan claim is what a label reads.
  const claim = Buffer.from(JSON.stringify({ 'https://api.openai.com/auth': { chatgpt_plan_type: plan } })).toString('base64url');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, 'auth.json'),
    JSON.stringify({ tokens: { access_token: `x.${claim}.y`, refresh_token: 'r', account_id: 'acct' } })
  );
};

test('a profile is its own CODEX_HOME, and no name means the machine\'s own', () => {
  expect(codexHomeFor('work')).toBe(path.join(ROOT, 'work'));
  expect(codexHomeFor('')).toBe(HOME);
});

test('a new profile starts as a copy of the machine\'s codex settings, without its login', () => {
  fs.writeFileSync(path.join(HOME, 'config.toml'), 'model = "gpt-5.6-sol"\n');
  fs.mkdirSync(path.join(HOME, 'prompts'), { recursive: true });
  fs.writeFileSync(path.join(HOME, 'prompts', 'review.md'), 'review this');
  authFile(HOME, 'prolite');

  const dir = createCodexProfile('fresh');
  expect(fs.readFileSync(path.join(dir, 'config.toml'), 'utf8')).toContain('gpt-5.6-sol');
  expect(fs.readFileSync(path.join(dir, 'prompts', 'review.md'), 'utf8')).toBe('review this');
  expect(fs.existsSync(path.join(dir, 'auth.json'))).toBe(false);

  const profile = listCodexProfiles().find((p) => p.name === 'fresh');
  expect(profile.authenticated).toBe(false);
  expect(plain(codexProfileLabel(profile))).toBe('fresh  logged out');
});

test('importing copies the machine\'s login in, and the label reports its plan', () => {
  const dir = importCodexProfile('imported');
  expect(fs.existsSync(path.join(dir, 'auth.json'))).toBe(true);

  const profile = listCodexProfiles().find((p) => p.name === 'imported');
  expect(profile.authenticated).toBe(true);
  expect(plain(codexProfileLabel(profile))).toBe('imported  prolite');
});

test('a rollout is staged at the same relative path inside the destination profile', () => {
  const source = path.join(HOME, 'sessions', '2026', '08', '01', 'rollout-2026-08-01T22-22-02-019fc047-4079-7c40-b6ba-e868044fadd8.jsonl');
  const session = { id: '019fc047-4079-7c40-b6ba-e868044fadd8', file: source };

  expect(codexSessionEntries(session, { sourceHome: HOME, targetHome: codexHomeFor('work') })).toEqual([
    { source, target: path.join(ROOT, 'work', 'sessions', '2026', '08', '01', path.basename(source)) }
  ]);
});

test('a rollout from outside the source profile, or with a bogus id, is refused', () => {
  const id = '019fc047-4079-7c40-b6ba-e868044fadd8';
  expect(() => codexSessionEntries({ id, file: path.join(os.tmpdir(), `rollout-${id}.jsonl`) }, { sourceHome: HOME, targetHome: ROOT }))
    .toThrow(/outside its Codex profile/);
  expect(() => codexSessionEntries({ id: 'not-a-uuid', file: 'x.jsonl' }, { sourceHome: HOME, targetHome: ROOT }))
    .toThrow(/Invalid Codex session id/);
  expect(() => codexSessionEntries({ id }, { sourceHome: HOME, targetHome: ROOT }))
    .toThrow(/no rollout path/);
});

test('removing a profile takes its directory with it', () => {
  createCodexProfile('doomed');
  expect(removeCodexProfile('doomed')).toBe(true);
  expect(fs.existsSync(codexHomeFor('doomed'))).toBe(false);
  expect(removeCodexProfile('doomed')).toBe(false);
  // Cleanup for the whole file — every profile above lived under ROOT.
  for (const dir of [ROOT, HOME]) {
    if (path.dirname(path.resolve(dir)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
