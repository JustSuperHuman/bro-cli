import { beforeEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { runCodex } from './codex.js';

// The profile root and the machine's codex home are pointed at temporary
// directories — no test may touch a real login. Both are resolved per call, so
// each test states them again: test files share one process, and a root left
// over from another file would reach its fixtures.
const ROOT = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-codex-run-'));
const HOME = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-codex-run-home-'));

beforeEach(() => {
  process.env.BRO_CODEX_PROFILES_DIR = ROOT;
  process.env.CODEX_HOME = HOME;
});

const ID = '019fc047-4079-7c40-b6ba-e868044fadd8';
const session = {
  kind: 'codex-session',
  id: ID,
  account: null,
  cwd: process.cwd(),
  title: 'fix the picker',
  file: path.join(HOME, 'sessions', '2026', '08', '01', `rollout-2026-08-01T22-22-02-${ID}.jsonl`)
};

test('a session resumes in place for its own login and forks into any other', async () => {
  const same = await runCodex({ session, dryRun: true });
  const cross = await runCodex({ session, profile: 'work', dryRun: true });

  expect(same.forkSession).toBe(false);
  expect(same.args.slice(0, 2)).toEqual(['resume', ID]);
  expect(same.profile).toBe('(this machine)');

  expect(cross.forkSession).toBe(true);
  expect(cross.args.slice(0, 2)).toEqual(['fork', ID]);
  expect(cross.profile).toBe('work');
  expect(cross.sourceProfile).toBe('(this machine)');
  expect(cross.codexHome).toBe(path.join(ROOT, 'work'));
});

test('a session already owned by the profile resuming it is not a fork', async () => {
  const owned = { ...session, account: 'work' };
  const plan = await runCodex({ session: owned, profile: 'work', dryRun: true });

  expect(plan.forkSession).toBe(false);
  expect(plan.args.slice(0, 2)).toEqual(['resume', ID]);
  expect(plan.codexHome).toBe(path.join(ROOT, 'work'));
});

test('the codex harness runs the CLI under the chosen login, with no bridge', async () => {
  const plan = await runCodex({ harness: 'codex', profile: 'work', dryRun: true });

  expect(plan.via).toBe('codex CLI (ChatGPT login)');
  expect(plan.codexHome).toBe(path.join(ROOT, 'work'));
  expect(plan.args).not.toContain('resume');

  for (const dir of [ROOT, HOME]) {
    if (path.dirname(path.resolve(dir)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
