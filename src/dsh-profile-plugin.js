import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  claudeCodeLoginStatus,
  startClaudeOAuthBridge
} from './claude-oauth-bridge.js';
import { fetchCodexModels, startCodexBridge } from './codex-bridge.js';
import { listCodexProfiles, localCodexProfile } from './codex-profiles.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export const DSH_PROFILES_PLUGIN_PACKAGE = '@bro-cli/dsh-profiles';
export const DSH_PROFILES_PLUGIN_ID = 'bro-profile-switcher';

const CLAUDE_MODELS = [
  { id: 'claude-sonnet-5', name: 'Claude Sonnet 5' },
  { id: 'claude-opus-5', name: 'Claude Opus 5' },
  { id: 'claude-haiku-4-5-20251001', name: 'Claude Haiku 4.5' }
];
const CODEX_MODELS = [
  { id: 'gpt-5.6-sol', name: 'GPT-5.6-Sol' },
  { id: 'gpt-5.6-terra', name: 'GPT-5.6-Terra' },
  { id: 'gpt-5.5', name: 'GPT-5.5' },
  { id: 'gpt-5.4-mini', name: 'GPT-5.4-Mini' }
];

const hash = (value, length = 8) => crypto
  .createHash('sha256')
  .update(String(value))
  .digest('hex')
  .slice(0, length);

const safeId = (value) => String(value || 'profile')
  .toLowerCase()
  .replace(/[^a-z0-9-]+/g, '-')
  .replace(/^-+|-+$/g, '') || 'profile';

const samePath = (left, right) => {
  if (!left || !right) return false;
  const a = path.resolve(left);
  const b = path.resolve(right);
  return process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b;
};

const modelRows = (models, fallback) => {
  const seen = new Set();
  const rows = [];
  for (const model of [...(models || []), ...fallback]) {
    if (!model?.id || seen.has(model.id)) continue;
    seen.add(model.id);
    rows.push({ id: model.id, name: model.name || model.id });
  }
  return rows;
};

const preferredModel = (models, preferred) =>
  models.find((model) => model.id === preferred)?.id || models[0]?.id || '';

function claudeProfiles() {
  const poolDir = process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
  const accountsDir = path.join(poolDir, 'accounts');
  let names = [];
  try {
    names = fs.readdirSync(accountsDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
  } catch {
    /* an empty/missing account root is a valid setup */
  }

  const stored = names.map((name) => {
    const configDir = path.join(accountsDir, name);
    return {
      id: `claude-${hash(configDir)}`,
      kind: 'claude',
      name,
      label: name,
      configDir,
      ...claudeCodeLoginStatus({ configDir })
    };
  });
  const configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
  if (!stored.some((profile) => samePath(profile.configDir, configDir))) {
    stored.unshift({
      id: `claude-${hash(configDir)}`,
      kind: 'claude',
      name: 'local',
      label: 'This machine',
      configDir,
      ...claudeCodeLoginStatus({ configDir })
    });
  }
  return stored;
}

function codexProfiles() {
  const local = localCodexProfile();
  return [
    {
      id: `codex-${hash(local.dir)}`,
      kind: 'codex',
      name: 'local',
      label: 'This machine',
      home: '',
      dir: local.dir,
      authenticated: local.authenticated,
      plan: local.plan
    },
    ...listCodexProfiles().map((profile) => ({
      id: `codex-${hash(profile.dir)}`,
      kind: 'codex',
      name: profile.name,
      label: profile.name,
      home: profile.dir,
      dir: profile.dir,
      authenticated: profile.authenticated,
      plan: profile.plan
    }))
  ];
}

export function dshProfileRoute(profile) {
  const source = profile.kind === 'claude' ? profile.configDir : profile.dir;
  return `bro-${profile.kind}-${safeId(profile.name)}-${hash(source, 6)}`;
}

function descriptor(profile, { route = '', models = [], available = false, routeError = false } = {}) {
  return {
    id: profile.id,
    kind: profile.kind,
    name: profile.name,
    label: profile.label,
    authenticated: Boolean(profile.authenticated),
    available: Boolean(available && route),
    route,
    defaultModel: preferredModel(
      models,
      profile.kind === 'claude' ? 'claude-sonnet-5' : 'gpt-5.6-sol'
    ),
    models: models.map((model) => model.id),
    plan: profile.subscriptionType || profile.plan || null,
    ...(routeError ? { routeError: true } : {}),
    usageSource: profile.kind === 'claude'
      ? { kind: 'claude', configDir: profile.configDir }
      : { kind: 'codex', home: profile.home }
  };
}

function profileProvider(profile, { route, baseUrl, models }) {
  return {
    id: `dsh-${profile.id}`,
    name: `${profile.kind === 'claude' ? 'Claude' : 'Codex'} · ${profile.label}`,
    mode: 'anthropic',
    baseUrl,
    noKey: false,
    disable1mContext: true,
    dshRoute: route,
    models
  };
}

export function dshProfilesPluginPatch(profiles) {
  return {
    insert: [{
      id: DSH_PROFILES_PLUGIN_ID,
      name: DSH_PROFILES_PLUGIN_PACKAGE,
      config: { profiles }
    }]
  };
}

// Prepare one real DSH route per authenticated login. A broken/logged-out
// profile remains visible in the plugin, but never prevents the other routes
// or DSH itself from launching.
export async function prepareDshProfilesPlugin({
  dryRun = false,
  nativeClaudeProvider = null,
  reuseClaude = null,
  reuseCodex = null,
  discoveredClaudeProfiles,
  discoveredCodexProfiles,
  startClaudeBridge = startClaudeOAuthBridge,
  getCodexModels = fetchCodexModels,
  startCodexProfileBridge = startCodexBridge,
  warn = (message) => console.warn(`\x1b[33m  ${message}\x1b[0m`)
} = {}) {
  const providers = [];
  const providerKeys = new Map();
  const stops = [];
  const claude = discoveredClaudeProfiles || claudeProfiles();
  const codex = discoveredCodexProfiles || codexProfiles();
  const fallbackClaudeModels = modelRows(nativeClaudeProvider?.models, CLAUDE_MODELS);

  const connectClaude = async (profile) => {
    if (!profile.authenticated) return descriptor(profile);
    if (reuseClaude && samePath(reuseClaude.configDir, profile.configDir)) {
      const models = modelRows(reuseClaude.models || reuseClaude.provider?.models, fallbackClaudeModels);
      return descriptor(profile, {
        route: reuseClaude.route || reuseClaude.provider?.dshRoute || 'anthropic',
        models,
        available: true
      });
    }
    const route = dshProfileRoute(profile);
    try {
      if (dryRun) {
        const provider = profileProvider(profile, {
          route,
          baseUrl: `http://127.0.0.1:<${profile.id}-bridge>`,
          models: fallbackClaudeModels
        });
        providers.push(provider);
        providerKeys.set(provider.id, 'sk-ant-oat-bro-dry-run');
        return descriptor(profile, { route, models: fallbackClaudeModels, available: true });
      }
      const bridge = await startClaudeBridge({ configDir: profile.configDir });
      const models = modelRows(bridge.models, fallbackClaudeModels);
      const provider = profileProvider(profile, { route, baseUrl: bridge.baseUrl, models });
      providers.push(provider);
      providerKeys.set(provider.id, bridge.apiKey);
      stops.push(() => bridge.stop());
      return descriptor(profile, { route, models, available: true });
    } catch (error) {
      warn(`Claude profile "${profile.label}" is visible but its DSH route could not start: ${error.message}`);
      return descriptor(profile, { routeError: true });
    }
  };

  const connectCodex = async (profile) => {
    if (!profile.authenticated) return descriptor(profile);
    const route = dshProfileRoute(profile);
    const canReuse = reuseCodex && (
      (!profile.home && !reuseCodex.home)
      || (profile.home && reuseCodex.home && samePath(profile.home, reuseCodex.home))
    );
    if (canReuse) {
      const models = modelRows(reuseCodex.models || reuseCodex.provider?.models, CODEX_MODELS);
      return descriptor(profile, {
        route: reuseCodex.route || reuseCodex.provider?.dshRoute || 'bro-codex',
        models,
        available: true
      });
    }
    try {
      const models = dryRun ? CODEX_MODELS : modelRows(await getCodexModels({ home: profile.home }), CODEX_MODELS);
      if (dryRun) {
        const provider = profileProvider(profile, {
          route,
          baseUrl: `http://127.0.0.1:<${profile.id}-bridge>`,
          models
        });
        providers.push(provider);
        providerKeys.set(provider.id, 'bro-codex');
        return descriptor(profile, { route, models, available: true });
      }
      const activeModel = preferredModel(models, 'gpt-5.6-sol');
      const bridge = await startCodexProfileBridge({
        port: 0,
        defaultModel: activeModel,
        models,
        home: profile.home
      });
      const provider = profileProvider(profile, { route, baseUrl: bridge.baseUrl, models });
      providers.push(provider);
      providerKeys.set(provider.id, 'bro-codex');
      stops.push(() => bridge.close());
      return descriptor(profile, { route, models, available: true });
    } catch (error) {
      warn(`Codex profile "${profile.label}" is visible but its DSH route could not start: ${error.message}`);
      return descriptor(profile, { routeError: true });
    }
  };

  const profiles = await Promise.all([
    ...claude.map(connectClaude),
    ...codex.map(connectCodex)
  ]);

  return {
    providers,
    providerKeys,
    profiles,
    patch: dshProfilesPluginPatch(profiles),
    connected: profiles.filter((profile) => profile.available).length,
    async stop() {
      await Promise.allSettled([...stops].reverse().map((stop) => stop()));
    }
  };
}

export function dshHomePath() {
  return path.resolve(process.env.DSH_HOME || path.join(os.homedir(), '.dsh'));
}

// DSH resolves out-of-tree plugins through its shared profiles/node_modules
// fallback. A junction keeps the bundled plugin in lockstep with every bro
// install/update without copying or mutating DSH's profile manifests.
export function ensureDshProfilesPlugin({
  home = dshHomePath(),
  pluginRoot = path.resolve(__dirname, '..', 'dsh-plugin-profiles')
} = {}) {
  const manifest = path.join(pluginRoot, 'package.json');
  if (!fs.existsSync(manifest)) throw new Error(`Bundled DSH profiles plugin is missing: ${manifest}`);
  const modulesRoot = path.resolve(home, 'profiles', 'node_modules');
  const link = path.join(modulesRoot, '@bro-cli', 'dsh-profiles');
  fs.mkdirSync(path.dirname(link), { recursive: true });

  let stat = null;
  try { stat = fs.lstatSync(link); } catch {}
  if (stat) {
    if (!stat.isSymbolicLink()) {
      throw new Error(
        `${link} already exists and is not bro's managed plugin link. ` +
        'Move or remove that directory, then launch DeepSeek Harness again.'
      );
    }
    let current = '';
    try { current = fs.realpathSync.native(link); } catch {}
    if (samePath(current, pluginRoot)) return { link, pluginRoot };
    fs.unlinkSync(link);
  }
  fs.symlinkSync(pluginRoot, link, process.platform === 'win32' ? 'junction' : 'dir');
  return { link, pluginRoot };
}
