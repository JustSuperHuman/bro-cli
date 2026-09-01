import { expect, test } from 'bun:test';
import {
  buildDshOverlay,
  dshCredentialEnv,
  dshModelFor,
  dshProviderProfile,
  launchDsh
} from './deepseek.js';

const deepseek = {
  id: 'deepseek',
  name: 'DeepSeek',
  mode: 'openai',
  baseUrl: 'https://api.deepseek.com/chat/completions',
  models: [
    { id: 'deepseek-chat', name: 'DeepSeek Chat' },
    { id: 'deepseek-reasoner', name: 'DeepSeek Reasoner' }
  ]
};

const openrouter = {
  id: 'openrouter',
  name: 'OpenRouter',
  mode: 'anthropic',
  baseUrl: 'https://openrouter.ai/api',
  models: [
    { id: 'openrouter/fusion', name: 'Fusion' },
    { id: 'moonshotai/kimi-k2.7-code', name: 'Kimi K2.7 Code' }
  ]
};

const noProfiles = async () => ({
  providers: [],
  providerKeys: new Map(),
  profiles: [],
  connected: 0,
  patch: null,
  stop: async () => {}
});

test('bro providers map to DSH native wire protocols and endpoint roots', () => {
  const openai = dshProviderProfile(deepseek);
  expect(openai.api).toBe('openai-completions');
  expect(openai.baseURL).toBe('https://api.deepseek.com');
  expect(openai.displayName).toBe('Bro · DeepSeek');
  expect(openai.models.map((entry) => entry.id)).toEqual(['deepseek-chat', 'deepseek-reasoner']);

  const anthropic = dshProviderProfile(openrouter);
  expect(anthropic.api).toBe('anthropic-messages');
  expect(anthropic.baseURL).toBe('https://openrouter.ai/api');
  expect(anthropic.models.map((entry) => entry.id)).toEqual([
    'openrouter/fusion',
    'moonshotai/kimi-k2.7-code'
  ]);
});

test('a model selected in bro is inserted without losing the rest of that provider catalog', () => {
  const profile = dshProviderProfile(openrouter, { selectedModel: 'brand-new/model' });
  expect(profile.models.map((entry) => entry.id)).toEqual([
    'brand-new/model',
    'openrouter/fusion',
    'moonshotai/kimi-k2.7-code'
  ]);
  expect(dshModelFor(openrouter, '')).toBe('openrouter/fusion');
});

test('the overlay namespaces bro routes, syncs all models, and never contains API keys', () => {
  const secret = 'sk-do-not-write-this';
  const overlay = buildDshOverlay({
    providers: [openrouter, deepseek],
    providerKeys: { openrouter: secret },
    selectedProvider: openrouter,
    selectedModel: 'moonshotai/kimi-k2.7-code'
  });
  const providerConfig = overlay.patch[0].config.providers;

  expect(Object.keys(providerConfig)).toEqual(['bro-openrouter', 'bro-deepseek']);
  expect(Object.keys(providerConfig)).not.toContain('openrouter');
  expect(overlay.selected).toEqual({
    provider: 'bro-openrouter',
    model: 'moonshotai/kimi-k2.7-code'
  });
  expect(overlay.patch[1]).toEqual({
    id: 'agent-default-model',
    config: { provider: 'bro-openrouter', model: 'moonshotai/kimi-k2.7-code' }
  });
  expect(providerConfig['bro-openrouter'].models).toHaveLength(2);
  expect(JSON.stringify(overlay.patch)).not.toContain(secret);
  expect(overlay.credentials[dshCredentialEnv(openrouter)]).toBe(secret);
});

test('DSH dry-run exposes provider/model sync and redacts every credential', async () => {
  const plan = await launchDsh({
    provider: deepseek,
    model: 'deepseek-reasoner',
    apiKey: 'sk-selected-secret',
    providers: [deepseek, openrouter],
    providerKeys: new Map([['openrouter', 'sk-other-secret']]),
    extraArgs: ['--port', '4080'],
    skipPermissions: false,
    dryRun: true,
    prepareProfiles: noProfiles
  });

  expect(plan.cmd).toBeTruthy();
  expect(plan.args).toEqual([
    'web', '--patch', '<temporary bro provider overlay>', '--port', '4080'
  ]);
  expect(plan.selected).toEqual({ provider: 'bro-deepseek', model: 'deepseek-reasoner' });
  expect(plan.syncedProviders).toEqual([
    { id: 'bro-deepseek', models: 2 },
    { id: 'bro-openrouter', models: 2 }
  ]);
  expect(plan.env.DSH_PERMISSION_MODE).toBe('workspace-write');
  expect(Object.values(plan.env)).not.toContain('sk-selected-secret');
  expect(Object.values(plan.env)).not.toContain('sk-other-secret');
  expect(JSON.stringify(plan)).not.toContain('sk-selected-secret');
  expect(JSON.stringify(plan)).not.toContain('sk-other-secret');
  expect(plan.profileSwitcher).toEqual({ autoloaded: true, profiles: 0, connected: 0 });
});

test('a profile chosen before launch becomes the fresh-session DSH default', async () => {
  const routeProvider = {
    id: 'dsh-claude-work',
    name: 'Claude · work',
    mode: 'anthropic',
    baseUrl: 'http://127.0.0.1:<work-bridge>',
    dshRoute: 'bro-claude-work',
    models: [{ id: 'claude-sonnet-5', name: 'Sonnet' }]
  };
  const prepareProfiles = async () => ({
    providers: [routeProvider],
    providerKeys: new Map([[routeProvider.id, 'sk-ant-oat-bro-dry-run']]),
    profiles: [{
      id: 'claude-work', kind: 'claude', name: 'work', label: 'work',
      authenticated: true, available: true, route: 'bro-claude-work',
      defaultModel: 'claude-sonnet-5', models: ['claude-sonnet-5']
    }],
    connected: 1,
    patch: null,
    stop: async () => {}
  });
  const plan = await launchDsh({
    provider: deepseek,
    model: 'deepseek-chat',
    providers: [deepseek],
    dryRun: true,
    prepareProfiles,
    preferredProfile: { kind: 'claude', name: 'work' }
  });

  expect(plan.selected).toEqual({ provider: 'bro-claude-work', model: 'claude-sonnet-5' });
  expect(plan.profileSwitcher.preferred).toEqual({
    kind: 'claude', name: 'work', route: 'bro-claude-work'
  });
  expect(JSON.stringify(plan)).not.toContain('sk-ant-oat-bro-dry-run');
});

test('sanitized provider-id collisions stay independently switchable', () => {
  const first = { ...deepseek, id: 'my gateway', name: 'First' };
  const second = { ...deepseek, id: 'my-gateway', name: 'Second' };
  const overlay = buildDshOverlay({ providers: [first, second], selectedProvider: first });
  const ids = Object.keys(overlay.patch[0].config.providers);

  expect(ids[0]).toBe('bro-my-gateway');
  expect(ids[1]).toMatch(/^bro-my-gateway-[a-f0-9]{8}$/);
  expect(new Set(ids).size).toBe(2);
});

test('a Claude Code subscription route stays switchable but is not an unusable fresh default', async () => {
  const native = {
    id: 'anthropic',
    name: 'Claude (Anthropic)',
    mode: 'native',
    models: [{ name: 'Default' }, { id: 'claude-sonnet-4-6', name: 'Claude Sonnet 4.6' }]
  };
  const withoutKey = buildDshOverlay({ providers: [native], selectedProvider: native });
  const withKey = buildDshOverlay({
    providers: [native],
    providerKeys: { anthropic: 'sk-ant-test' },
    selectedProvider: native
  });

  expect(withoutKey.patch[0].config.providers.anthropic).toBeDefined();
  expect(withoutKey.selected).toBeNull();
  expect(withoutKey.patch).toHaveLength(1);
  expect(withKey.selected).toEqual({ provider: 'anthropic', model: 'claude-sonnet-4-6' });

  const plan = await launchDsh({
    provider: native,
    providers: [native],
    dryRun: true,
    prepareProfiles: noProfiles,
    loginStatus: () => ({
      authenticated: true,
      configDir: 'C:\\Users\\me\\.claude',
      subscriptionType: 'max'
    })
  });
  expect(plan.selected).toEqual({ provider: 'anthropic', model: 'claude-sonnet-4-6' });
  expect(plan.claudeOAuth).toMatchObject({
    via: 'Claude Code OAuth → refresh-safe loopback Anthropic bridge',
    subscriptionType: 'max',
    models: 1
  });
  expect(plan.note).toBeUndefined();
  expect(JSON.stringify(plan)).not.toContain('sk-ant-oat-bro-dry-run');

  const loggedOut = await launchDsh({
    provider: native,
    providers: [native],
    dryRun: true,
    prepareProfiles: noProfiles,
    loginStatus: () => ({ authenticated: false, configDir: '/missing', subscriptionType: null })
  });
  expect(loggedOut.note).toMatch(/No Claude Code OAuth login was found/);
});
