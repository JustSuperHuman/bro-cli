import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { accountLabel, runAccountProfile, stageSessionForProfile, usageSummary } from './pool.js';

test('profile usage preserves the limits users use to choose an account', () => {
  const usage = usageSummary({
    five_hour: { utilization: 17 },
    seven_day: { utilization: 28 },
    limits: [
      { kind: 'weekly_scoped', percent: 53, scope: { model: { display_name: 'Fable' } } }
    ]
  });

  expect(usage).toEqual({ session: 17, weekly: 28, fable: 53 });
  const plain = accountLabel({ name: 'James', authenticated: true, subscriptionType: 'max', usageStats: usage })
    .replace(/\x1b\[[0-9;]*m/g, '');
  expect(plain).toContain('5h 17% · wk 28% · Fable 53%');
});

test('a profile whose usage is still loading keeps the row shape with placeholders', () => {
  const label = accountLabel({ name: 'James', authenticated: true, subscriptionType: 'max', usagePending: true })
    .replace(/\x1b\[[0-9;]*m/g, '');
  expect(label).toBe('James  5h … · wk … · Fable … · max');
});

test('a stats outage does not hide or disable an authenticated profile', () => {
  const label = accountLabel({ name: 'work', authenticated: true, subscriptionType: 'team', usageStats: null });
  expect(label).toContain('work');
  expect(label).toContain('usage unavailable');
  expect(label).toContain('team');
});

test('cross-profile resume stages a disposable transcript copy and keeps the source', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-session-stage-'));
  const source = path.join(root, 'source');
  const target = path.join(root, 'target');
  const project = 'F--bro-cli';
  const id = '11111111-2222-4333-8444-555555555555';
  const sourceFile = path.join(source, 'projects', project, `${id}.jsonl`);
  const sourceEnv = path.join(source, 'session-env', id);

  try {
    fs.mkdirSync(path.dirname(sourceFile), { recursive: true });
    fs.mkdirSync(sourceEnv, { recursive: true });
    fs.writeFileSync(sourceFile, '{"type":"user"}\n');
    fs.writeFileSync(path.join(sourceEnv, 'env'), 'TEST=1');

    const staged = stageSessionForProfile(
      { id, file: sourceFile },
      { sourceConfigDir: source, targetConfigDir: target }
    );
    const targetFile = path.join(target, 'projects', project, `${id}.jsonl`);
    expect(fs.readFileSync(targetFile, 'utf8')).toBe('{"type":"user"}\n');
    expect(fs.readFileSync(path.join(target, 'session-env', id, 'env'), 'utf8')).toBe('TEST=1');

    staged.cleanup();
    expect(fs.existsSync(targetFile)).toBe(false);
    expect(fs.existsSync(path.join(target, 'session-env', id))).toBe(false);
    expect(fs.existsSync(sourceFile)).toBe(true);
  } finally {
    if (path.dirname(path.resolve(root)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('cross-profile dry runs fork while owner-profile resumes continue in place', async () => {
  const session = {
    id: '11111111-2222-4333-8444-555555555555',
    account: 'source',
    cwd: process.cwd(),
    title: 'test session',
    file: 'unused-in-dry-run.jsonl'
  };
  const same = await runAccountProfile({ accountName: 'source', session, dryRun: true });
  const cross = await runAccountProfile({ accountName: 'target', session, dryRun: true });

  expect(same.forkSession).toBe(false);
  expect(same.claude.args).not.toContain('--fork-session');
  expect(cross.forkSession).toBe(true);
  expect(cross.claude.args).toContain('--fork-session');
  expect(cross.account).toBe('target');
});
