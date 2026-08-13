import { expect, test } from 'bun:test';
import { codexProviderConfig, launchCodex } from './launch.js';

const deepseek = { id: 'deepseek', name: 'DeepSeek', mode: 'openai', baseUrl: 'https://api.deepseek.com/chat/completions' };
const codex = { id: 'codex', name: 'Codex (ChatGPT subscription)', mode: 'codex' };

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
