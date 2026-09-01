import { afterEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import {
  dshProfileRoute,
  ensureDshProfilesPlugin,
  prepareDshProfilesPlugin
} from './dsh-profile-plugin.js';
import {
  createProfileCatalog,
  publicProfile
} from '../dsh-plugin-profiles/index.js';
import { startCodexBridge } from './codex-bridge.js';

const tempRoots = [];
afterEach(() => {
  for (const root of tempRoots.splice(0)) fs.rmSync(root, { recursive: true, force: true });
});

test('every authenticated Claude and Codex login gets an independent DSH route', async () => {
  const stopped = [];
  const calls = [];
  const claude = [
    {
      id: 'claude-a', kind: 'claude', name: 'work', label: 'work',
      configDir: 'C:\\profiles\\claude-work', authenticated: true, subscriptionType: 'max'
    },
    {
      id: 'claude-b', kind: 'claude', name: 'old', label: 'old',
      configDir: 'C:\\profiles\\claude-old', authenticated: false
    }
  ];
  const codex = [
    {
      id: 'codex-a', kind: 'codex', name: 'personal', label: 'personal',
      home: 'C:\\profiles\\codex-personal', dir: 'C:\\profiles\\codex-personal',
      authenticated: true, plan: 'pro'
    },
    {
      id: 'codex-b', kind: 'codex', name: 'old', label: 'old',
      home: 'C:\\profiles\\codex-old', dir: 'C:\\profiles\\codex-old', authenticated: false
    }
  ];

  const setup = await prepareDshProfilesPlugin({
    discoveredClaudeProfiles: claude,
    discoveredCodexProfiles: codex,
    startClaudeBridge: async () => ({
      baseUrl: 'http://127.0.0.1:4101',
      apiKey: 'secret-claude-bridge-key',
      models: [{ id: 'claude-sonnet-5', name: 'Sonnet' }],
      stop: async () => stopped.push('claude')
    }),
    getCodexModels: async ({ home }) => {
      calls.push(home);
      return [{ id: 'gpt-5.6-sol', name: 'Sol' }];
    },
    startCodexProfileBridge: async (options) => ({
      baseUrl: 'http://127.0.0.1:4102',
      close: async () => stopped.push('codex'),
      ...options
    })
  });

  expect(setup.connected).toBe(2);
  expect(setup.profiles).toHaveLength(4);
  expect(setup.providers).toHaveLength(2);
  expect(setup.profiles.find((profile) => profile.id === 'claude-b')).toMatchObject({
    authenticated: false, available: false
  });
  expect(setup.profiles.find((profile) => profile.id === 'codex-b')).toMatchObject({
    authenticated: false, available: false
  });
  expect(setup.profiles.find((profile) => profile.id === 'claude-a').route).toBe(dshProfileRoute(claude[0]));
  expect(setup.profiles.find((profile) => profile.id === 'codex-a').route).toBe(dshProfileRoute(codex[0]));
  expect(calls).toEqual(['C:\\profiles\\codex-personal']);
  expect(setup.patch.insert[0]).toMatchObject({
    id: 'bro-profile-switcher', name: '@bro-cli/dsh-profiles'
  });
  expect(JSON.stringify(setup.patch)).not.toContain('secret-claude-bridge-key');

  await setup.stop();
  expect(stopped.sort()).toEqual(['claude', 'codex']);
});

test('an already-running Codex bridge is reused for its matching profile', async () => {
  const profile = {
    id: 'codex-local', kind: 'codex', name: 'local', label: 'This machine',
    home: '', dir: 'C:\\Users\\me\\.codex', authenticated: true, plan: 'pro'
  };
  const setup = await prepareDshProfilesPlugin({
    discoveredClaudeProfiles: [],
    discoveredCodexProfiles: [profile],
    reuseCodex: {
      home: '',
      route: 'bro-codex',
      models: [{ id: 'gpt-5.6-sol', name: 'Sol' }]
    },
    getCodexModels: async () => { throw new Error('should not fetch twice'); },
    startCodexProfileBridge: async () => { throw new Error('should not start twice'); }
  });

  expect(setup.providers).toHaveLength(0);
  expect(setup.connected).toBe(1);
  expect(setup.profiles[0]).toMatchObject({
    available: true,
    route: 'bro-codex',
    defaultModel: 'gpt-5.6-sol'
  });
});

test('the browser catalog strips credential paths and degrades usage failures per profile', async () => {
  const profiles = [
    {
      id: 'claude-a', kind: 'claude', name: 'work', label: 'Work', authenticated: true,
      available: true, route: 'bro-claude-work', defaultModel: 'claude-sonnet-5',
      models: ['claude-sonnet-5'], plan: 'max',
      usageSource: { kind: 'claude', configDir: 'C:\\private\\claude' }
    },
    {
      id: 'codex-a', kind: 'codex', name: 'work', label: 'Work', authenticated: true,
      available: true, route: 'bro-codex-work', defaultModel: 'gpt-5.6-sol',
      models: ['gpt-5.6-sol'], plan: 'pro',
      usageSource: { kind: 'codex', home: 'C:\\private\\codex' }
    }
  ];
  const catalog = createProfileCatalog(profiles, {
    claudeUsage: async () => ({ session: 20, weekly: 30, fable: 40 }),
    codexUsage: async () => { throw new Error('offline'); },
    now: () => 1_000
  });
  const loaded = await catalog.load();
  const serialized = JSON.stringify(loaded);

  expect(loaded.profiles[0].usage).toEqual({ session: 20, weekly: 30, fable: 40 });
  expect(loaded.profiles[1]).toMatchObject({ usage: null, usageError: true });
  expect(serialized).not.toContain('C:\\private');
  expect(publicProfile(profiles[0])).not.toHaveProperty('usageSource');
});

test('the bundled DSH plugin link is persistent and idempotent', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-dsh-plugin-test-'));
  tempRoots.push(root);
  const home = path.join(root, 'home');
  const pluginRoot = path.join(root, 'plugin');
  fs.mkdirSync(pluginRoot, { recursive: true });
  fs.writeFileSync(path.join(pluginRoot, 'package.json'), '{}');

  const first = ensureDshProfilesPlugin({ home, pluginRoot });
  const second = ensureDshProfilesPlugin({ home, pluginRoot });
  expect(first).toEqual(second);
  expect(fs.lstatSync(first.link).isSymbolicLink()).toBe(true);
  expect(fs.realpathSync(first.link)).toBe(fs.realpathSync(pluginRoot));
});

test('Codex profile bridges accept an ephemeral port', async () => {
  const bridge = await startCodexBridge({
    port: 0,
    defaultModel: 'gpt-test',
    models: [{ id: 'gpt-test', name: 'Test' }]
  });
  try {
    expect(bridge.port).toBeGreaterThan(0);
    expect((await fetch(`${bridge.baseUrl}/health`)).ok).toBe(true);
  } finally {
    await bridge.close();
  }
});

test('the DSH client bundle mounts a footer action and switches through the shared model directory', () => {
  const source = fs.readFileSync(path.join(process.cwd(), 'dsh-plugin-profiles', 'client.js'), 'utf8');
  expect(source).toContain('sidebar.footer.action');
  expect(source).toContain('modelDirectories.directoryFor');
  expect(source).toContain('directory.select({ provider: profile.route, model })');
  expect(source).toContain('/bro-profiles');
});
