import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  which,
  globalBinDirs,
  runInherit,
  ensureProxy,
  ensureClaude,
  ensureCodex,
  ensureOmp,
  ensurePi
} from './proc.js';
import { launchDsh } from './deepseek.js';
import { note } from './out.js';
import { browserBackend, claudeBrowserEnabled, prepareClaudeBrowser, usesThirdPartyAuth } from './claude-browser.js';
import { CHROME_MCP_SERVER_NAME } from './chrome-mcp.js';
import { MCP_CHROME_SERVER_NAME } from './mcp-chrome-server.js';

// What a dry run should say about the browser: the built-in flag for a
// claude.ai login, the explicitly configured server for everything else.
const describeBrowserWiring = (env, bridged = usesThirdPartyAuth(env)) =>
  browserBackend() === 'mcp-chrome'
    ? `mcp__${MCP_CHROME_SERVER_NAME}__* (--mcp-config, mcp-chrome extension)`
    : bridged ? `mcp__${CHROME_MCP_SERVER_NAME}__* (--mcp-config)` : '--chrome';

const CCR_CONFIG = path.join(os.homedir(), '.claude-code-router', 'config.json');
const OMP_MODELS = path.join(os.homedir(), '.omp', 'agent', 'models.yml');
export const PI_MODELS = path.join(os.homedir(), '.pi', 'agent', 'models.json');
const PI_KEY_ENV = 'BRO_PI_API_KEY';
export const piModelsPath = () => process.env.PI_CODING_AGENT_DIR
  ? path.join(process.env.PI_CODING_AGENT_DIR, 'models.json')
  : PI_MODELS;

function yamlString(value) {
  return JSON.stringify(String(value ?? ''));
}

function normalizeOpenAiBaseUrl(baseUrl) {
  return String(baseUrl || '').replace(/\/chat\/completions\/?$/i, '').replace(/\/responses\/?$/i, '');
}

function ompApiFor(provider) {
  if (provider.mode === 'anthropic' || provider.mode === 'native') return 'anthropic-messages';
  return 'openai-completions';
}

export function ompModelSelector(provider, model) {
  if (!model) return '';
  return `${provider.id}/${model}`;
}

function ompProviderBlock(provider, model, apiKey) {
  const lines = [`  ${provider.id}:`];
  const baseUrl = provider.mode === 'openai' ? normalizeOpenAiBaseUrl(provider.baseUrl) : provider.baseUrl;
  if (baseUrl) lines.push(`    baseUrl: ${yamlString(baseUrl)}`);
  lines.push(`    api: ${ompApiFor(provider)}`);
  if (provider.noKey) {
    lines.push('    auth: none');
  } else {
    lines.push(`    apiKey: ${yamlString(apiKey || provider.keyEnv || '')}`);
    if (provider.mode === 'openai') lines.push('    authHeader: true');
  }
  if (provider.disable1mContext || provider.mode === 'anthropic') lines.push('    disableStrictTools: true');
  const models = (provider.models || [])
    .map((m) => m && m.id)
    .filter(Boolean);
  if (model && !models.includes(model)) models.unshift(model);
  if (models.length) {
    lines.push('    models:');
    for (const id of models) {
      const info = (provider.models || []).find((m) => m.id === id) || {};
      lines.push(`      - id: ${yamlString(id)}`);
      if (info.name) lines.push(`        name: ${yamlString(info.name)}`);
    }
  }
  return lines.join('\n');
}

function findProvidersSection(lines) {
  const start = lines.findIndex((line) => /^providers:\s*$/.test(line));
  if (start < 0) return null;
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    if (/^\S/.test(lines[i]) && !/^providers:\s*$/.test(lines[i])) {
      end = i;
      break;
    }
  }
  return { start, end };
}

function upsertOmpProvider(id, block) {
  let text = '';
  try {
    text = fs.readFileSync(OMP_MODELS, 'utf8');
  } catch {
    /* fresh file */
  }
  const lines = text ? text.replace(/\r\n/g, '\n').split('\n') : [];
  let section = findProvidersSection(lines);
  if (!section) {
    if (lines.length && lines[lines.length - 1] !== '') lines.push('');
    lines.push('providers:');
    section = { start: lines.length - 1, end: lines.length };
  }

  const next = [];
  for (let i = section.start + 1; i < section.end; i++) {
    const match = /^  ([A-Za-z0-9_.-]+):\s*$/.exec(lines[i]);
    if (match && match[1] === id) {
      i++;
      while (i < section.end && !/^  [A-Za-z0-9_.-]+:\s*$/.test(lines[i])) i++;
      i--;
      continue;
    }
    next.push(lines[i]);
  }
  while (next.length && next[next.length - 1] === '') next.pop();
  next.push(...block.split('\n'));

  const merged = [
    ...lines.slice(0, section.start + 1),
    ...next,
    ...lines.slice(section.end)
  ].join('\n').replace(/\n{3,}/g, '\n\n');
  fs.mkdirSync(path.dirname(OMP_MODELS), { recursive: true });
  fs.writeFileSync(OMP_MODELS, merged.endsWith('\n') ? merged : `${merged}\n`);
}

export function writeOmpConfig(provider, model, apiKey) {
  if (provider.mode === 'native') return;
  upsertOmpProvider(provider.id, ompProviderBlock(provider, model, apiKey));
}

export async function launchOmp({ provider, model, apiKey, extraArgs = [], skipPermissions = true, dryRun = false }) {
  const ompArgs = [];
  if (skipPermissions) ompArgs.push('--yolo');
  const selector = ompModelSelector(provider, model);
  if (selector) ompArgs.push('--model', selector);
  ompArgs.push(...extraArgs);

  if (dryRun) {
    return {
      via: 'omp',
      cmd: which('omp', globalBinDirs()) || 'omp',
      args: ompArgs,
      ompModels: OMP_MODELS,
      model: selector || '(omp default)'
    };
  }

  writeOmpConfig(provider, model, apiKey);
  const { omp, dirs } = ensureOmp();
  const env = { ...process.env, PATH: [...dirs, process.env.PATH || ''].join(path.delimiter) };
  note(`\nLaunching ${provider.name || provider.id}${model ? ' / ' + model : ''} with omp…`);
  return runInherit(omp, ompArgs, env);
}

// --- Pi harness ------------------------------------------------------------

function piApiFor(provider) {
  if (provider.mode === 'anthropic' || provider.mode === 'native') return 'anthropic-messages';
  return 'openai-completions';
}

// Keep bro-managed providers in their own namespace so an OpenRouter/OpenAI
// entry cannot replace Pi's built-in provider or the user's own overrides.
export function piProviderId(provider) {
  if (provider.mode === 'native') return 'anthropic';
  const safe = String(provider.id || 'provider')
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'provider';
  return `bro-${safe}`;
}

export function piModelFor(provider, model) {
  return model || (provider.models || []).find((entry) => entry?.id)?.id || '';
}

function piConfiguredModelId(model, knownIds) {
  if (!model || knownIds.includes(model)) return model;
  return model.replace(/:(?:off|minimal|low|medium|high|xhigh|max)$/i, '');
}

export function piProviderEntry(provider, model) {
  const ids = (provider.models || []).map((entry) => entry?.id).filter(Boolean);
  const configuredModel = piConfiguredModelId(model, ids);
  if (configuredModel && !ids.includes(configuredModel)) ids.unshift(configuredModel);
  const baseUrl = provider.mode === 'openai' ? normalizeOpenAiBaseUrl(provider.baseUrl) : provider.baseUrl;
  return {
    ...(baseUrl ? { baseUrl } : {}),
    api: piApiFor(provider),
    // Pi resolves this as an environment variable at request time. The actual
    // provider key therefore never lands in models.json or the process list.
    apiKey: PI_KEY_ENV,
    authHeader: true,
    models: ids.map((id) => {
      const info = (provider.models || []).find((entry) => entry?.id === id) || {};
      return { id, ...(info.name ? { name: info.name } : {}) };
    })
  };
}

export function writePiConfig(provider, model, modelsPath = piModelsPath()) {
  if (provider.mode === 'native') return;

  let config = {};
  if (fs.existsSync(modelsPath)) {
    try {
      config = JSON.parse(fs.readFileSync(modelsPath, 'utf8'));
    } catch (error) {
      throw new Error(`Could not update Pi's provider config because ${modelsPath} is not valid JSON: ${error.message}`);
    }
  }
  if (!config || Array.isArray(config) || typeof config !== 'object') {
    throw new Error(`Could not update Pi's provider config because ${modelsPath} must contain a JSON object.`);
  }
  if (config.providers != null && (Array.isArray(config.providers) || typeof config.providers !== 'object')) {
    throw new Error(`Could not update Pi's provider config because its "providers" value must be an object.`);
  }

  config.providers = { ...(config.providers || {}), [piProviderId(provider)]: piProviderEntry(provider, model) };
  fs.mkdirSync(path.dirname(modelsPath), { recursive: true });
  fs.writeFileSync(modelsPath, `${JSON.stringify(config, null, 2)}\n`);
}

export async function launchPi({ provider, model, apiKey, extraArgs = [], dryRun = false }) {
  const providerId = piProviderId(provider);
  // Pi only applies --provider when --model is also present. Resolve a blank
  // bro "default" row to the provider's first concrete model so it cannot
  // silently restore a model from a different provider.
  const activeModel = piModelFor(provider, model);
  if (!activeModel) {
    throw new Error(`Pi needs a concrete model for ${provider.name || provider.id}. Pass --model or add models to the provider config.`);
  }
  const piArgs = ['--provider', providerId];
  if (activeModel) piArgs.push('--model', activeModel);
  piArgs.push(...extraArgs);

  if (dryRun) {
    return {
      via: provider.mode === 'native' ? 'pi (native provider)' : `pi → ${provider.name || provider.id}`,
      cmd: which('pi', globalBinDirs()) || 'pi',
      args: piArgs,
      ...(provider.mode !== 'native' ? { piModels: piModelsPath(), env: { [PI_KEY_ENV]: apiKey ? '(api key)' : '(not needed)' } } : {}),
      model: activeModel || '(pi default)'
    };
  }

  writePiConfig(provider, activeModel);
  const { pi, dirs } = ensurePi();
  const env = { ...process.env, PATH: [...dirs, process.env.PATH || ''].join(path.delimiter) };
  if (provider.mode !== 'native') env[PI_KEY_ENV] = apiKey || 'not-needed';
  note(`\nLaunching ${provider.name || provider.id}${activeModel ? ' / ' + activeModel : ''} with Pi…`);
  return runInherit(pi, piArgs, env);
}

// --- codex CLI harness -----------------------------------------------------

// The provider slot bro describes to codex, and the env var it reads the key
// from. Both only exist for the life of one launch: `codex -c key=value`
// overrides config in memory, so ~/.codex/config.toml is never touched.
const CODEX_PROVIDER_SLOT = 'bro';
const CODEX_KEY_ENV = 'BRO_PROVIDER_API_KEY';

// TOML-quote a value so codex parses it as a string rather than guessing —
// bare URLs and names with spaces are not valid TOML on their own.
const toml = (value) => JSON.stringify(String(value ?? ''));

// Describe the chosen provider to codex. Codex talks to its own ChatGPT login
// (the codex provider, which needs no description at all) or to any endpoint
// serving OpenAI's Responses API — as of codex 0.147 the older
// /chat/completions wire format is refused outright, so a provider that only
// offers that will be turned away by codex itself. Anthropic-shaped providers
// — the native Claude login, the account pool, OpenRouter/Z.ai via
// ANTHROPIC_BASE_URL — have no route in at all and are refused here, before
// anything is spawned.
export function codexProviderConfig(provider, apiKey) {
  if (provider.mode === 'codex') return { args: [], env: {} };
  if (provider.mode !== 'openai') {
    throw new Error(
      `The codex harness can't run ${provider.name || provider.id}: it speaks the Anthropic API, and codex only talks to OpenAI-compatible endpoints.\n` +
        '  Use the claude or omp harness for this provider (press h in the picker, or pass --claude / --omp),\n' +
        '  or choose "Codex (ChatGPT subscription)" to run codex on your ChatGPT login.'
    );
  }
  const args = [
    '-c', `model_provider=${toml(CODEX_PROVIDER_SLOT)}`,
    '-c', `model_providers.${CODEX_PROVIDER_SLOT}.name=${toml(provider.name || provider.id)}`,
    '-c', `model_providers.${CODEX_PROVIDER_SLOT}.base_url=${toml(normalizeOpenAiBaseUrl(provider.baseUrl))}`,
    '-c', `model_providers.${CODEX_PROVIDER_SLOT}.wire_api=${toml('responses')}`
  ];
  const env = {};
  // A key is named, never inlined: codex reads it from the environment, so it
  // stays out of the process list. Local providers that need none say so by
  // leaving env_key unset.
  if (apiKey) {
    args.push('-c', `model_providers.${CODEX_PROVIDER_SLOT}.env_key=${toml(CODEX_KEY_ENV)}`);
    env[CODEX_KEY_ENV] = apiKey;
  }
  return { args, env };
}

// Launch the codex CLI.
//   home         the login profile's CODEX_HOME (blank = this machine's own)
//   resume/fork  continue an existing rollout by id, or fork it into a new one
//                — forking is what a cross-profile resume does, so the source
//                login's conversation is never continued in place
//   cwd          run in that session's project instead of here
// Skipping permissions is codex's sandbox+approval bypass, the same bargain
// --dangerously-skip-permissions makes for Claude Code.
export async function launchCodex({
  provider,
  model,
  apiKey,
  extraArgs = [],
  skipPermissions = true,
  home = '',
  profile = '',
  resume = '',
  resumeTitle = '',
  sourceProfile = '',
  fork = false,
  cwd = '',
  dryRun = false
}) {
  const { args: providerArgs, env: providerEnv } = codexProviderConfig(provider, apiKey);
  const codexArgs = [];
  if (resume) codexArgs.push(fork ? 'fork' : 'resume', resume);
  if (skipPermissions) codexArgs.push('--dangerously-bypass-approvals-and-sandbox');
  if (model) codexArgs.push('--model', model);
  codexArgs.push(...providerArgs, ...extraArgs);

  if (dryRun) {
    return {
      via: provider.mode === 'codex' ? 'codex CLI (ChatGPT login)' : `codex CLI → ${provider.name || provider.id}`,
      cmd: which('codex', globalBinDirs()) || 'codex',
      args: codexArgs,
      ...(home ? { codexHome: home } : {}),
      ...(providerEnv[CODEX_KEY_ENV] ? { env: { [CODEX_KEY_ENV]: '(api key)' } } : {}),
      ...(resume ? { resume, cwd: cwd || process.cwd() } : {}),
      model: model || '(codex default)'
    };
  }

  const { codex, dirs } = ensureCodex();
  if (cwd && !fs.existsSync(cwd)) throw new Error(`That session's directory is gone: ${cwd}`);

  const env = {
    ...process.env,
    ...providerEnv,
    NODE_NO_WARNINGS: '1',
    PATH: [...dirs, process.env.PATH || ''].join(path.delimiter)
  };
  // A profile is a whole codex home: credentials, sessions and settings. No
  // profile means the machine's own, so the user's own CODEX_HOME stands.
  if (home) env.CODEX_HOME = home;

  const title = resumeTitle.length > 60 ? resumeTitle.slice(0, 59) + '…' : resumeTitle;
  const named = title ? ` “${title}”` : '';
  const as = profile ? ` as ${profile}` : '';
  const banner = resume
    ? fork
      ? `Forking Codex session${named} from ${sourceProfile || 'this machine'} and resuming${as || ' locally'}`
      : `Resuming Codex session${named}${as}`
    : `Launching Codex${provider.mode === 'codex' ? as : ' / ' + (provider.name || provider.id)}`;
  note(`\n${banner}${model ? ' / ' + model : ''}${cwd ? `\nin ${cwd}` : ''}…`);
  if (provider.mode !== 'codex') {
    note(`\x1b[2m  codex calls ${normalizeOpenAiBaseUrl(provider.baseUrl)}/responses — a provider that only serves /chat/completions will refuse it.\x1b[0m`);
  }
  return runInherit(codex, codexArgs, env, { ...(cwd ? { cwd } : {}), terminalAgent: 'codex' });
}

// Upsert this provider into the proxy's config and point its default route at the
// chosen model. Existing (hand-edited) providers in the file are preserved.
function writeCcrConfig(provider, model, apiKey) {
  let cfg = {};
  try {
    cfg = JSON.parse(fs.readFileSync(CCR_CONFIG, 'utf8'));
  } catch {
    /* fresh file */
  }
  cfg.LOG = cfg.LOG ?? false;
  cfg.API_TIMEOUT_MS = cfg.API_TIMEOUT_MS ?? 600000;
  cfg.Providers = Array.isArray(cfg.Providers) ? cfg.Providers : [];

  const entry = {
    name: provider.id,
    api_base_url: provider.baseUrl,
    api_key: apiKey || 'not-needed',
    models: (provider.models || []).map((m) => m.id).filter(Boolean)
  };
  if (model && !entry.models.includes(model)) entry.models.push(model);

  const i = cfg.Providers.findIndex((p) => p.name === provider.id);
  if (i >= 0) cfg.Providers[i] = entry;
  else cfg.Providers.push(entry);

  cfg.Router = cfg.Router || {};
  cfg.Router.default = `${provider.id},${model}`;

  fs.mkdirSync(path.dirname(CCR_CONFIG), { recursive: true });
  fs.writeFileSync(CCR_CONFIG, JSON.stringify(cfg, null, 2));
}

// Launch the chosen harness for the chosen provider/model. With the claude
// harness:
//   native    -> run claude with the user's own login
//   anthropic -> point claude at an Anthropic-compatible base URL
//   openai    -> route claude through the proxy (ccr)
// With { dryRun: true } nothing is spawned or written; returns a description.
export async function launch({
  provider,
  model,
  apiKey,
  providers = [],
  providerKeys = {},
  extraArgs = [],
  skipPermissions = true,
  harness = 'claude',
  // No terminal is watching, so the browser is joined but never raised.
  headless = false,
  dryRun = false,
  preferredProfile = null
}) {
  if (harness === 'dsh') {
    return launchDsh({
      provider,
      model,
      apiKey,
      providers,
      providerKeys,
      extraArgs,
      skipPermissions,
      dryRun,
      preferredProfile
    });
  }
  if (harness === 'omp') {
    return launchOmp({ provider, model, apiKey, extraArgs, skipPermissions, dryRun });
  }
  if (harness === 'pi') {
    return launchPi({ provider, model, apiKey, extraArgs, dryRun });
  }
  if (harness === 'codex') {
    return launchCodex({ provider, model, apiKey, extraArgs, skipPermissions, dryRun });
  }

  const claudeArgs = [];
  if (skipPermissions) claudeArgs.push('--dangerously-skip-permissions');
  const browserEnabled = claudeBrowserEnabled()
    && !extraArgs.includes('--no-chrome')
    && !extraArgs.includes('--chrome');
  if (model) claudeArgs.push('--model', provider.mode === 'openai' ? `${provider.id},${model}` : model);
  claudeArgs.push(...extraArgs);

  if (provider.mode === 'openai') {
    if (dryRun) {
      return {
        via: 'proxy (claude-code-router)',
        cmd: which('ccr', globalBinDirs()) || 'ccr',
        args: ['code', ...claudeArgs],
        ccrConfig: CCR_CONFIG,
        route: `${provider.id},${model}`,
        ...(browserEnabled ? { browser: describeBrowserWiring(null, true) } : {})
      };
    }
    writeCcrConfig(provider, model, apiKey);
    const { claude, dirs: claudeDirs } = ensureClaude();
    const { ccr, dirs } = ensureProxy();
    const env = { ...process.env, NODE_NO_WARNINGS: '1' };
    for (const k of ['ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_API_KEY', 'CLAUDE_CONFIG_DIR', 'CLAUDE_CODE_DISABLE_1M_CONTEXT']) {
      delete env[k];
    }
    env.PATH = [...dirs, ...claudeDirs, env.PATH].join(path.delimiter);
    // The proxy sets its own credentials on the session it spawns, so this
    // route always ends up on non-claude.ai auth however clean bro's env looks.
    const browser = browserEnabled
      ? await prepareClaudeBrowser({ claudePath: claude, baseEnv: env, skipPermissions, bridged: true, autoStart: !headless })
      : null;
    if (browser) Object.assign(env, browser.env);
    note(`\nLaunching ${provider.name || provider.id} / ${model} via the proxy…`);
    return runInherit(ccr, ['code', ...(browser?.args || []), ...claudeArgs], env, { terminalAgent: 'claude' });
  }

  // native + anthropic-compatible both run the claude CLI directly.
  const env = { ...process.env };
  if (provider.mode === 'anthropic') {
    env.ANTHROPIC_BASE_URL = provider.baseUrl;
    env.ANTHROPIC_AUTH_TOKEN = apiKey || '';
    env.ANTHROPIC_API_KEY = '';
    if (provider.disable1mContext) env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
    if (provider.env) Object.assign(env, provider.env);
  }

  if (dryRun) {
    return {
      via: provider.mode === 'native' ? 'native Claude' : 'anthropic-compatible',
      cmd: which('claude', globalBinDirs()) || 'claude',
      args: claudeArgs,
      baseUrl: provider.mode === 'anthropic' ? provider.baseUrl : '(default)',
      ...(browserEnabled ? { browser: describeBrowserWiring(env) } : {})
    };
  }

  const { claude, dirs } = ensureClaude();
  env.PATH = [...dirs, env.PATH || ''].join(path.delimiter);
  // A native login gets Claude Code's own --chrome; an Anthropic-compatible
  // provider gets the same browser through the MCP server, because the
  // ANTHROPIC_AUTH_TOKEN set just above switches the built-in wiring off.
  const browser = browserEnabled
    ? await prepareClaudeBrowser({ claudePath: claude, baseEnv: env, skipPermissions, autoStart: !headless })
    : null;
  if (browser) Object.assign(env, browser.env);
  note(`\nLaunching ${provider.name || provider.id}${model ? ' / ' + model : ''}…`);
  return runInherit(claude, [...(browser?.args || []), ...claudeArgs], env, { terminalAgent: 'claude' });
}
