import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  codexProviderConfig,
  codexResponsesBaseUrl,
  launchCodex,
  launchPi,
  piModelFor,
  piProviderEntry,
  piProviderId,
  writePiConfig
} from './launch.js';

const deepseek = { id: 'deepseek', name: 'DeepSeek', mode: 'openai', baseUrl: 'https://api.deepseek.com/chat/completions' };
const codex = { id: 'codex', name: 'Codex (ChatGPT subscription)', mode: 'codex' };
const openrouter = {
  id: 'openrouter',
  name: 'OpenRouter',
  mode: 'anthropic',
  baseUrl: 'https://openrouter.ai/api',
  models: [{ id: 'stealth/union-alpha' }]
};

const configOf = (args) => Object.fromEntries(
  args.filter((a, i) => args[i - 1] === '-c').map((entry) => {
    const at = entry.indexOf('=');
    return [entry.slice(0, at), JSON.parse(entry.slice(at + 1))];
  })
);

test('the ChatGPT subscription needs no provider config — codex uses its own login', () => {
  expect(codexProviderConfig(codex, '')).toEqual({ args: [], env: {} });
});

test('an OpenAI-compatible provider is described to codex for one run, key held in the env', () => {
  const { args, env } = codexProviderConfig(deepseek, 'sk-test');
  const config = configOf(args);

  expect(config.model_provider).toBe('bro');
  // codex appends the wire path itself, so the endpoint suffix has to come off.
  expect(config['model_providers.bro.base_url']).toBe('https://api.deepseek.com');
  // codex 0.147 refuses the older /chat/completions wire format outright.
  expect(config['model_providers.bro.wire_api']).toBe('responses');
  expect(config['model_providers.bro.env_key']).toBe('BRO_PROVIDER_API_KEY');
  expect(env.BRO_PROVIDER_API_KEY).toBe('sk-test');
  // The key is never spelled out in the arguments, which are world-readable.
  expect(args.join(' ')).not.toContain('sk-test');
});

test('OpenRouter uses its Responses API route when selected with the codex harness', () => {
  expect(codexResponsesBaseUrl(openrouter)).toBe('https://openrouter.ai/api/v1');
  const { args, env } = codexProviderConfig(openrouter, 'sk-or-test');
  const config = configOf(args);

  expect(config['model_providers.bro.base_url']).toBe('https://openrouter.ai/api/v1');
  expect(config['model_providers.bro.wire_api']).toBe('responses');
  expect(env.BRO_PROVIDER_API_KEY).toBe('sk-or-test');
});

test('a provider that needs no key names no env var, so codex does not demand one', () => {
  const { args, env } = codexProviderConfig({ id: 'ollama', mode: 'openai', baseUrl: 'http://localhost:11434/v1/chat/completions' }, '');
  expect(args.join(' ')).not.toContain('env_key');
  expect(env).toEqual({});
});

test('an Anthropic-shaped provider is refused before anything is spawned', () => {
  expect(() => codexProviderConfig({ id: 'zai', name: 'Z.ai (GLM)', mode: 'anthropic', baseUrl: 'https://api.z.ai/api/anthropic' }, 'k'))
    .toThrow(/codex harness can't run Z\.ai/);
  expect(() => codexProviderConfig({ id: 'anthropic', name: 'Claude', mode: 'native' }, ''))
    .toThrow(/only talks to OpenAI-compatible endpoints/);
});

test('resuming a session continues that rollout in its own project directory', async () => {
  const plan = await launchCodex({
    provider: codex,
    model: '',
    resume: '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c',
    cwd: 'J:\\justgains',
    dryRun: true
  });

  expect(plan.args.slice(0, 2)).toEqual(['resume', '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c']);
  expect(plan.cwd).toBe('J:\\justgains');
  expect(plan.args).toContain('--dangerously-bypass-approvals-and-sandbox');
  // No profile means no CODEX_HOME: the user's own environment stays in charge.
  expect(plan.codexHome).toBeUndefined();
});

test('another profile forks the session rather than continuing it in place', async () => {
  const plan = await launchCodex({
    provider: codex,
    home: 'C:\\Users\\me\\.bro\\codex-profiles\\work',
    profile: 'work',
    resume: '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c',
    fork: true,
    dryRun: true
  });

  expect(plan.args.slice(0, 2)).toEqual(['fork', '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c']);
  expect(plan.codexHome).toBe('C:\\Users\\me\\.bro\\codex-profiles\\work');
});

test('--safe leaves codex its sandbox and approval prompts', async () => {
  const plan = await launchCodex({ provider: codex, model: 'gpt-5.6-codex', skipPermissions: false, dryRun: true });
  expect(plan.args).not.toContain('--dangerously-bypass-approvals-and-sandbox');
  expect(plan.args).toEqual(['--model', 'gpt-5.6-codex']);
});

test('Pi gets a namespaced custom provider and never persists the provider key', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-pi-models-'));
  const file = path.join(root, 'models.json');
  const provider = {
    ...deepseek,
    models: [{ id: 'deepseek-chat', name: 'DeepSeek Chat' }]
  };

  try {
    fs.writeFileSync(file, JSON.stringify({ keep: true, providers: { personal: { api: 'openai-completions' } } }));
    writePiConfig(provider, 'deepseek-reasoner', file);
    const config = JSON.parse(fs.readFileSync(file, 'utf8'));
    const entry = config.providers['bro-deepseek'];

    expect(config.keep).toBe(true);
    expect(config.providers.personal).toEqual({ api: 'openai-completions' });
    expect(entry.baseUrl).toBe('https://api.deepseek.com');
    expect(entry.api).toBe('openai-completions');
    expect(entry.apiKey).toBe('BRO_PI_API_KEY');
    expect(entry.authHeader).toBe(true);
    expect(entry.models.map((model) => model.id)).toEqual(['deepseek-reasoner', 'deepseek-chat']);
    expect(piProviderEntry(provider, 'deepseek-reasoner:high').models[0].id).toBe('deepseek-reasoner');
    expect(fs.readFileSync(file, 'utf8')).not.toContain('sk-test');
  } finally {
    if (path.dirname(path.resolve(root)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('Pi uses built-in Anthropic natively and custom Anthropic endpoints through bro', async () => {
  const native = {
    id: 'anthropic',
    name: 'Claude',
    mode: 'native',
    models: [{ name: 'Default' }, { id: 'claude-opus-4-8' }]
  };
  const openrouter = {
    id: 'openrouter',
    name: 'OpenRouter',
    mode: 'anthropic',
    baseUrl: 'https://openrouter.ai/api',
    models: [{ id: 'anthropic/claude-opus-4.6' }]
  };
  expect(piProviderId(native)).toBe('anthropic');
  expect(piProviderId(openrouter)).toBe('bro-openrouter');
  expect(piModelFor(native, '')).toBe('claude-opus-4-8');
  expect(piProviderEntry(openrouter, 'anthropic/claude-opus-4.6').api).toBe('anthropic-messages');

  const nativePlan = await launchPi({ provider: native, model: '', dryRun: true });
  expect(nativePlan.args).toEqual(['--provider', 'anthropic', '--model', 'claude-opus-4-8']);
  const routedPlan = await launchPi({
    provider: openrouter,
    model: 'anthropic/claude-opus-4.6',
    apiKey: 'secret-key',
    extraArgs: ['--continue'],
    dryRun: true
  });
  expect(routedPlan.args).toEqual([
    '--provider', 'bro-openrouter',
    '--model', 'anthropic/claude-opus-4.6',
    '--continue'
  ]);
  expect(routedPlan.env).toEqual({ BRO_PI_API_KEY: '(api key)' });
  expect(JSON.stringify(routedPlan)).not.toContain('secret-key');
});

test('an invalid Pi models file is preserved instead of overwritten', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-pi-invalid-'));
  const file = path.join(root, 'models.json');
  try {
    fs.writeFileSync(file, '{ definitely not json');
    expect(() => writePiConfig(deepseek, 'deepseek-chat', file)).toThrow(/not valid JSON/);
    expect(fs.readFileSync(file, 'utf8')).toBe('{ definitely not json');
  } finally {
    if (path.dirname(path.resolve(root)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('Pi refuses an empty custom provider instead of restoring an unrelated model', async () => {
  await expect(launchPi({
    provider: { id: 'empty', name: 'Empty provider', mode: 'openai', baseUrl: 'http://localhost:1234/v1' },
    model: '',
    dryRun: true
  })).rejects.toThrow(/needs a concrete model/);
});
