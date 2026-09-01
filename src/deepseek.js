import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  claudeCodeLoginStatus,
  claudeOAuthBridgeProvider,
  startClaudeOAuthBridge
} from './claude-oauth-bridge.js';
import {
  ensureDshProfilesPlugin,
  prepareDshProfilesPlugin
} from './dsh-profile-plugin.js';
import { ensureDsh, globalBinDirs, openExternalUrl, runInherit, runInheritObserved, which } from './proc.js';

const DSH_NATIVE_ANTHROPIC_URL = 'https://api.anthropic.com';

const safeId = (value) => String(value || 'provider')
  .toLowerCase()
  .replace(/[^a-z0-9-]+/g, '-')
  .replace(/^-+|-+$/g, '') || 'provider';

const shortHash = (value) => crypto.createHash('sha256').update(String(value)).digest('hex').slice(0, 8);

export function dshProviderId(provider) {
  if (provider?.dshRoute) return String(provider.dshRoute);
  if (provider?.mode === 'native' && provider?.id === 'anthropic') return 'anthropic';
  return `bro-${safeId(provider?.id)}`;
}

export function dshCredentialEnv(provider) {
  return `BRO_DSH_${safeId(provider?.id).replace(/-/g, '_').toUpperCase()}_${shortHash(provider?.id)}_API_KEY`;
}

export function dshModelFor(provider, model) {
  return model || (provider?.models || []).find((entry) => entry?.id)?.id || '';
}

function normalizeOpenAiBaseUrl(baseUrl) {
  return String(baseUrl || '')
    .replace(/\/chat\/completions\/?$/i, '')
    .replace(/\/responses\/?$/i, '');
}

function normalizeAnthropicBaseUrl(baseUrl) {
  return String(baseUrl || '').replace(/\/v1\/messages\/?$/i, '');
}

function dshApiFor(provider) {
  if (provider.mode === 'native' || provider.mode === 'anthropic') return 'anthropic-messages';
  if (provider.mode === 'openai') return 'openai-completions';
  return '';
}

function dshBaseUrlFor(provider) {
  if (provider.mode === 'native') return DSH_NATIVE_ANTHROPIC_URL;
  if (provider.mode === 'anthropic') return normalizeAnthropicBaseUrl(provider.baseUrl);
  if (provider.mode === 'openai') return normalizeOpenAiBaseUrl(provider.baseUrl);
  return '';
}

function configuredModels(provider, selectedModel = '') {
  const seen = new Set();
  const out = [];
  const add = (id, name = '') => {
    if (!id || seen.has(id)) return;
    seen.add(id);
    out.push({ id, ...(name ? { name } : {}) });
  };
  add(selectedModel, (provider.models || []).find((entry) => entry?.id === selectedModel)?.name);
  for (const entry of provider.models || []) add(entry?.id, entry?.name);
  return out;
}

export function dshProviderProfile(provider, { selectedModel = '' } = {}) {
  const api = dshApiFor(provider);
  const baseURL = dshBaseUrlFor(provider);
  const models = configuredModels(provider, selectedModel);
  if (!api || !baseURL || !models.length) {
    throw new Error(
      `DeepSeek Harness cannot describe ${provider.name || provider.id}: ` +
        'it needs an Anthropic/OpenAI-compatible endpoint and at least one concrete model.'
    );
  }
  return {
    displayName: `Bro · ${provider.name || provider.id}`,
    api,
    baseURL,
    ...(!provider.noKey ? { apiKeyEnv: dshCredentialEnv(provider) } : {}),
    models
  };
}

const keyFor = (keys, id) => keys instanceof Map ? keys.get(id) : keys?.[id];

const setKey = (keys, id, value) => {
  if (keys instanceof Map) keys.set(id, value);
  else keys[id] = value;
};

const isNativeClaude = (provider) => provider?.mode === 'native' && provider?.id === 'anthropic';

async function prepareClaudeOAuth({
  provider,
  providers,
  keys,
  dryRun,
  loginStatus,
  startBridge
}) {
  const native = [provider, ...providers].find(isNativeClaude);
  if (!native) return { provider, providers, bridge: null, bridgeInfo: null };

  const login = loginStatus();
  if (!login.authenticated) {
    return { provider, providers, bridge: null, bridgeInfo: null, login };
  }

  const bridge = dryRun ? null : await startBridge({ configDir: login.configDir });
  const baseUrl = bridge?.baseUrl || 'http://127.0.0.1:<ephemeral-claude-oauth-bridge>';
  const bridgeProvider = claudeOAuthBridgeProvider(native, {
    baseUrl,
    models: bridge?.models || native.models
  });
  // The dry-run marker has the same prefix pi-ai uses to select its OAuth
  // request dialect, but it is never passed to a child process.
  setKey(keys, native.id, bridge?.apiKey || 'sk-ant-oat-bro-dry-run');
  const selected = isNativeClaude(provider) ? bridgeProvider : provider;
  const mergedProviders = providers.map((entry) => isNativeClaude(entry) ? bridgeProvider : entry);
  if (!mergedProviders.some((entry) => entry?.id === bridgeProvider.id)) mergedProviders.unshift(bridgeProvider);
  return {
    provider: selected,
    providers: mergedProviders,
    bridge,
    bridgeInfo: {
      via: 'Claude Code OAuth → refresh-safe loopback Anthropic bridge',
      configDir: login.configDir,
      subscriptionType: bridge?.account?.subscriptionType || login.subscriptionType || null,
      baseUrl,
      models: bridge?.models?.length || bridgeProvider.models.length
    },
    login
  };
}

// Build one ephemeral composition overlay. Bro routes are namespaced, so DSH's
// richer installed provider catalog and every user-defined provider remain
// intact. DSH settings layer over this composition base, which also means a
// model chosen in DSH keeps winning on later launches.
export function buildDshOverlay({
  providers = [],
  providerKeys = {},
  selectedProvider,
  selectedModel = ''
} = {}) {
  const ordered = [selectedProvider, ...providers].filter(Boolean);
  const seenProviders = new Set();
  const usedRoutes = new Map();
  const routeByProvider = new Map();
  const profiles = {};
  const credentials = {};

  for (const provider of ordered) {
    const id = String(provider.id || '');
    if (!id || seenProviders.has(id)) continue;
    seenProviders.add(id);
    if (!['native', 'anthropic', 'openai'].includes(provider.mode)) continue;

    let routeId = dshProviderId(provider);
    const owner = usedRoutes.get(routeId);
    if (owner && owner !== id) routeId = `${routeId}-${shortHash(id)}`;
    usedRoutes.set(routeId, id);

    try {
      profiles[routeId] = dshProviderProfile(provider, {
        selectedModel: provider === selectedProvider ? selectedModel : ''
      });
    } catch (error) {
      if (provider === selectedProvider) throw error;
      continue;
    }
    routeByProvider.set(id, routeId);
    const key = keyFor(providerKeys, id);
    if (!provider.noKey && key) credentials[dshCredentialEnv(provider)] = key;
  }

  const patch = [{ id: 'llm-pi-ai', config: { providers: profiles } }];
  const activeModel = selectedProvider ? dshModelFor(selectedProvider, selectedModel) : '';
  const activeRoute = selectedProvider ? routeByProvider.get(String(selectedProvider.id || '')) : '';
  // launchDsh normally turns a Claude Code login into a credentialed loopback
  // route before reaching this pure builder. Keep the lower-level guard so a
  // caller that bypasses that preparation cannot create a broken default.
  const nativeWithoutApiKey = selectedProvider?.mode === 'native'
    && !keyFor(providerKeys, String(selectedProvider.id || ''));
  const selected = activeRoute && activeModel && !nativeWithoutApiKey
    ? { provider: activeRoute, model: activeModel }
    : null;
  if (selected) {
    // This is the composition default for a fresh DSH home. The harness's own
    // saved model selection intentionally layers over it.
    patch.push({ id: 'agent-default-model', config: { provider: activeRoute, model: activeModel } });
  }

  return {
    patch,
    credentials,
    selected,
    providers: Object.entries(profiles).map(([id, profile]) => ({ id, models: profile.models.length }))
  };
}

function temporaryOverlay(patch) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-dsh-'));
  const file = path.join(root, 'bro.patch.json');
  fs.writeFileSync(file, `${JSON.stringify(patch, null, 2)}\n`, { mode: 0o600 });
  return {
    file,
    cleanup: () => fs.rmSync(root, { recursive: true, force: true })
  };
}

function urlObserver(onUrl) {
  let buffer = '';
  let opened = false;
  return (chunk) => {
    if (opened) return;
    buffer = `${buffer}${chunk}`.slice(-8192);
    const match = /dsh web:\s+(https?:\/\/[^\s)]+)/i.exec(buffer);
    if (!match) return;
    opened = true;
    onUrl(match[1]);
  };
}

export async function launchDsh({
  provider,
  model,
  apiKey = '',
  providers = [],
  providerKeys = {},
  extraArgs = [],
  skipPermissions = true,
  dryRun = false,
  loginStatus = claudeCodeLoginStatus,
  startBridge = startClaudeOAuthBridge,
  prepareProfiles = prepareDshProfilesPlugin,
  installProfilesPlugin = ensureDshProfilesPlugin,
  reuseCodex = null,
  preferredProfile = null
}) {
  const keys = providerKeys instanceof Map ? new Map(providerKeys) : { ...(providerKeys || {}) };
  if (provider?.id && apiKey) {
    setKey(keys, provider.id, apiKey);
  }
  let prepared;
  let profileSetup;
  try {
    try {
      prepared = await prepareClaudeOAuth({
        provider,
        providers,
        keys,
        dryRun,
        loginStatus,
        startBridge
      });
    } catch (error) {
      const selectedNative = isNativeClaude(provider);
      if (selectedNative && !keyFor(keys, String(provider.id || ''))) throw error;
      if (!dryRun) console.warn(`\x1b[33m  Claude OAuth route unavailable: ${error.message}\x1b[0m`);
      prepared = { provider, providers, bridge: null, bridgeInfo: null };
    }

    const activeProvider = prepared.provider;
    const bridgedClaude = [prepared.provider, ...prepared.providers]
      .find((entry) => entry?.id === 'anthropic' && entry?.mode === 'anthropic' && entry?.dshRoute === 'anthropic');
    const reuseClaude = prepared.login?.authenticated && bridgedClaude
      ? {
          configDir: prepared.login.configDir,
          route: dshProviderId(bridgedClaude),
          provider: bridgedClaude,
          models: bridgedClaude.models
        }
      : null;
    profileSetup = await prepareProfiles({
      dryRun,
      nativeClaudeProvider: [provider, ...providers].find(isNativeClaude) || null,
      reuseClaude,
      reuseCodex: reuseCodex
        ? {
            ...reuseCodex,
            route: reuseCodex.route || (reuseCodex.provider ? dshProviderId(reuseCodex.provider) : '')
          }
        : null
    });
    for (const [id, key] of profileSetup.providerKeys || []) setKey(keys, id, key);
    const activeProviders = [...prepared.providers, ...(profileSetup.providers || [])];
    const overlay = buildDshOverlay({
      providers: activeProviders,
      providerKeys: keys,
      selectedProvider: activeProvider,
      selectedModel: model
    });
    const preferred = preferredProfile
      ? profileSetup.profiles.find((profile) =>
          profile.kind === preferredProfile.kind && profile.name === preferredProfile.name
        )
      : null;
    if (preferred?.available && preferred.route && preferred.defaultModel) {
      const selected = { provider: preferred.route, model: preferred.defaultModel };
      const defaultModelPatch = overlay.patch.find((entry) => entry.id === 'agent-default-model');
      if (defaultModelPatch) defaultModelPatch.config = selected;
      else overlay.patch.push({ id: 'agent-default-model', config: selected });
      overlay.selected = selected;
    }
    if (profileSetup.patch) overlay.patch.push(profileSetup.patch);
    const executable = which('dsh', globalBinDirs()) || 'dsh';
    const displayArgs = ['web', '--patch', '<temporary bro provider overlay>', ...extraArgs];
    const autoOpen = process.env.BRO_DSH_NO_OPEN !== '1'
      && !extraArgs.some((arg) => ['--help', '-h', '--dump-config', '--dump-default-config'].includes(arg));
    const nativeSubscriptionOnly = activeProvider?.mode === 'native'
      && !keyFor(keys, String(activeProvider.id || ''));
    if (dryRun) {
      return {
        via: 'DeepSeek Harness Web UI',
        cmd: executable,
        args: displayArgs,
        selected: overlay.selected,
        syncedProviders: overlay.providers,
        env: {
          DSH_PERMISSION_MODE: skipPermissions ? 'danger-full-access' : 'workspace-write',
          ...Object.fromEntries(Object.keys(overlay.credentials).map((name) => [name, '(api key)']))
        },
        opensBrowser: autoOpen,
        profileSwitcher: {
          autoloaded: true,
          profiles: profileSetup.profiles?.length || 0,
          connected: profileSetup.connected || 0,
          ...(preferredProfile ? {
            preferred: preferred?.available
              ? { kind: preferred.kind, name: preferred.name, route: preferred.route }
              : { kind: preferredProfile.kind, name: preferredProfile.name, unavailable: true }
          } : {})
        },
        ...(prepared.bridgeInfo ? { claudeOAuth: prepared.bridgeInfo } : {}),
        ...(nativeSubscriptionOnly ? {
          note: 'No Claude Code OAuth login was found. Run `claude` and complete /login, or configure ANTHROPIC_API_KEY.'
        } : {})
      };
    }

    const { dsh, dirs } = ensureDsh();
    installProfilesPlugin();
    const temp = temporaryOverlay(overlay.patch);
    const env = {
      ...process.env,
      ...overlay.credentials,
      DSH_PERMISSION_MODE: skipPermissions ? 'danger-full-access' : 'workspace-write',
      PATH: [...dirs, process.env.PATH || ''].join(path.delimiter)
    };
    const args = ['web', '--patch', temp.file, ...extraArgs];
    const totalModels = overlay.providers.reduce((sum, entry) => sum + entry.models, 0);
    console.log(
      `\nLaunching DeepSeek Harness Web with ${overlay.providers.length} bro provider${overlay.providers.length === 1 ? '' : 's'} / ` +
        `${totalModels} model${totalModels === 1 ? '' : 's'} synced…`
    );
    if (preferredProfile && !preferred?.available) {
      console.warn(
        `\x1b[33m  Requested ${preferredProfile.kind} profile "${preferredProfile.name}" is not logged in or its route is unavailable; choose another in Profiles.\x1b[0m`
      );
    }
    if (overlay.selected) {
      console.log(`\x1b[2m  fresh-profile default: ${overlay.selected.provider} / ${overlay.selected.model}\x1b[0m`);
    }
    if (prepared.bridgeInfo) {
      console.log(
        `\x1b[2m  Claude Code OAuth: connected${prepared.bridgeInfo.subscriptionType ? ` (${prepared.bridgeInfo.subscriptionType})` : ''}` +
          ` · ${prepared.bridgeInfo.models} model${prepared.bridgeInfo.models === 1 ? '' : 's'}\x1b[0m`
      );
    } else if (nativeSubscriptionOnly) {
      console.warn(
        '\x1b[33m  No Claude Code OAuth login found. Run `claude` and complete /login, or configure ANTHROPIC_API_KEY.\x1b[0m'
      );
    }
    console.log(
      `\x1b[2m  Profiles plugin: ${profileSetup.profiles.length} Claude/Codex login${profileSetup.profiles.length === 1 ? '' : 's'} found` +
        ` · ${profileSetup.connected} selectable\x1b[0m`
    );
    console.log(
      `\x1b[2m  DSH keeps its native providers and its own saved model selection${autoOpen ? '; the browser opens when ready' : ''}.\x1b[0m\n`
    );

    const observe = urlObserver((url) => {
      console.log(`\x1b[2mOpening ${url}\x1b[0m`);
      openExternalUrl(url);
    });
    try {
      return autoOpen
        ? await runInheritObserved(dsh, args, env, { onStdout: observe })
        : await runInherit(dsh, args, env);
    } finally {
      temp.cleanup();
    }
  } finally {
    await profileSetup?.stop?.();
    await prepared?.bridge?.stop();
  }
}
