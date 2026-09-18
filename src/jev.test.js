import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { ensureJevKey, jevEnv, jevKeyStatus, jevNotice, jevSupport, JEV_COMMANDS, JEV_KEY_ID } from './jev.js';
import { jevPlanFor, launchCodex, launch } from './launch.js';
import { parseArgs, JEV_TOGGLE } from './cli.js';
import { normalizeKeyed } from './ui.js';

const native = { id: 'anthropic', name: 'Anthropic', mode: 'native' };
const account = { id: 'account', name: 'Claude account profile', mode: 'account' };
const codex = { id: 'codex', name: 'Codex (ChatGPT subscription)', mode: 'codex' };
const glm = { id: 'zai', name: 'Z.ai', mode: 'anthropic', baseUrl: 'https://api.z.ai/api/anthropic' };
const pool = { id: 'pool', name: 'Account pool', mode: 'pool' };

test('jev fronts Claude Code on its own login and on account profiles', () => {
  expect(jevSupport({ harness: 'claude', provider: native })).toEqual({ ok: true, command: 'jev-claude' });
  expect(jevSupport({ harness: 'claude', provider: account }).command).toBe('jev-claude');
});

test('jev fronts the codex CLI only on the ChatGPT login', () => {
  expect(jevSupport({ harness: 'codex', provider: codex }).command).toBe(JEV_COMMANDS.codex);
  const refused = jevSupport({ harness: 'codex', provider: glm });
  expect(refused.ok).toBe(false);
  expect(refused.reason).toContain('ChatGPT login');
});

test('a provider with a base URL of its own cannot be fronted', () => {
  for (const provider of [glm, pool, codex]) {
    const support = jevSupport({ harness: 'claude', provider });
    expect(support.ok).toBe(false);
    expect(support.hint).toBeTruthy();
  }
});

test('other harnesses are named rather than silently ignored', () => {
  const support = jevSupport({ harness: 'omp', provider: native });
  expect(support.ok).toBe(false);
  expect(support.reason).toContain('omp');
});

test('an unsupported route explains itself and runs unrouted', () => {
  const said = [];
  expect(jevPlanFor({ enabled: true, harness: 'claude', provider: glm, announce: (m) => said.push(m) })).toBe(null);
  expect(said.join('\n')).toContain('Running without Jev Router');
  expect(jevPlanFor({ enabled: false, harness: 'claude', provider: native, announce: () => {} })).toBe(null);
  expect(jevPlanFor({ enabled: true, harness: 'claude', provider: native, announce: () => {} }).ok).toBe(true);
});

test("the key is found in the environment, in bro's config, or in jev-router's own env file", () => {
  const none = { keys: {} };
  expect(jevKeyStatus({ env: { JEV_API_KEY: 'k' }, files: [], config: none }))
    .toEqual({ found: true, source: 'JEV_API_KEY', key: 'k' });
  expect(jevKeyStatus({ env: { TYPESAFE_API_KEY: 'k' }, files: [], config: none }).source).toBe('TYPESAFE_API_KEY');

  // A key bro asked for and saved carries its value, because bro has to put it
  // in the environment jev-router reads.
  const saved = jevKeyStatus({ env: {}, files: [], config: { keys: { [JEV_KEY_ID]: 'saved-key' } } });
  expect(saved.found).toBe(true);
  expect(saved.key).toBe('saved-key');
  expect(jevEnv(saved)).toEqual({ JEV_API_KEY: 'saved-key' });

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-jev-'));
  const file = path.join(dir, '.jev-router.env');
  fs.writeFileSync(file, '# comment\nJEV_API_KEY=abc123\n');
  const fromFile = jevKeyStatus({ env: {}, files: [file], config: none });
  expect(fromFile).toEqual({ found: true, source: file, key: '' });
  // jev-router reads that file itself, so bro has nothing to pass along.
  expect(jevEnv(fromFile)).toEqual({});

  fs.writeFileSync(file, 'JEV_API_KEY=\n');
  expect(jevKeyStatus({ env: {}, files: [file], config: none }).found).toBe(false);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('a missing key is asked for once and saved', async () => {
  const saved = [];
  const said = [];
  const status = await ensureJevKey({
    harness: 'claude',
    provider: native,
    status: { found: false, source: '', key: '', files: [] },
    ask: async () => '  typesafe-key  ',
    announce: (m) => said.push(m),
    save: (id, key) => saved.push([id, key])
  });
  expect(saved).toEqual([[JEV_KEY_ID, 'typesafe-key']]);
  expect(status.found).toBe(true);
  expect(status.key).toBe('typesafe-key');
  expect(said.join(' ')).toContain('docs.typesafe.ai');
});

test('a blank answer runs unrouted instead of saving an empty key', async () => {
  const saved = [];
  const status = await ensureJevKey({
    harness: 'claude',
    provider: native,
    status: { found: false, source: '', key: '', files: [] },
    ask: async () => '',
    announce: () => {},
    save: (id, key) => saved.push([id, key])
  });
  expect(saved).toEqual([]);
  expect(status.found).toBe(false);
});

test('a route Jev cannot front is never asked for a key', async () => {
  const saved = [];
  let asked = false;
  await ensureJevKey({
    harness: 'claude',
    provider: glm,
    ask: async () => { asked = true; return 'k'; },
    announce: () => {},
    save: (id, key) => saved.push([id, key])
  });
  expect(asked).toBe(false);
  expect(saved).toEqual([]);
});

test('a headless run says what is missing rather than waiting for a prompt', async () => {
  const said = [];
  let asked = false;
  const status = await ensureJevKey({
    harness: 'codex',
    provider: codex,
    interactive: false,
    status: { found: false, source: '', key: '', files: [] },
    ask: async () => { asked = true; return 'k'; },
    announce: (m) => said.push(m),
    save: () => {}
  });
  expect(asked).toBe(false);
  expect(status.found).toBe(false);
  expect(said.join(' ')).toContain('JEV_API_KEY');
});

test('a missing key is reported before the session opens', () => {
  const notice = jevNotice('claude');
  expect(notice).toContain('Jev Router');
});

test('--jev and --no-jev are bro flags, not harness arguments', () => {
  expect(parseArgs(['--jev']).jev).toBe(true);
  expect(parseArgs(['--jev-router']).jev).toBe(true);
  expect(parseArgs(['--no-jev']).jev).toBe(false);
  expect(parseArgs([]).jev).toBeUndefined();
  expect(parseArgs(['--jev'])._).toEqual([]);
});

test('a jev-fronted codex run spawns jev-codex and leaves the model to Jev', async () => {
  const out = await launchCodex({ provider: codex, model: 'gpt-5.6-sol', jev: true, dryRun: true });
  expect(out.cmd).toContain('jev-codex');
  expect(out.args).not.toContain('--model');
  expect(out.model).toContain('Jev');
  expect(out.jev.via).toContain('jev-codex');
});

test('a jev-fronted Claude run spawns jev-claude, keeping every other flag', async () => {
  const out = await launch({
    provider: native,
    model: 'claude-opus-5',
    harness: 'claude',
    jev: true,
    permissionMode: 'bypass',
    extraArgs: ['--resume', 'abc'],
    dryRun: true
  });
  expect(out.cmd).toContain('jev-claude');
  expect(out.args).toContain('--dangerously-skip-permissions');
  expect(out.args).toContain('--resume');
  expect(out.args).not.toContain('--model');
  expect(out.via).toContain('jev-router');
});

test('without the switch nothing changes', async () => {
  const out = await launch({ provider: native, model: 'claude-opus-5', harness: 'claude', permissionMode: 'bypass', dryRun: true });
  expect(out.cmd).not.toContain('jev');
  expect(out.args).toContain('--model');
  expect(out.jev).toBeUndefined();
});

test('the picker switch avoids the lists own vim keys', () => {
  const toggle = JEV_TOGGLE(true);
  expect(toggle.key).toBe('r');
  expect(['j', 'k', 'h', 'b']).not.toContain(toggle.key);
  expect(toggle.value).toBe('on');
  expect(JEV_TOGGLE(false).value).toBe('off');
  expect(normalizeKeyed([toggle])[0].index).toBe(1);
});
