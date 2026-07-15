import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { which, globalBinDirs, runInherit, ensureProxy, ensureOmp } from './proc.js';

const CCR_CONFIG = path.join(os.homedir(), '.claude-code-router', 'config.json');
const OMP_MODELS = path.join(os.homedir(), '.omp', 'agent', 'models.yml');

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
  console.log(`\nLaunching ${provider.name || provider.id}${model ? ' / ' + model : ''} with omp…`);
  return runInherit(omp, ompArgs, env);
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

// Launch claude for the chosen provider/model.
//   native    -> run claude with the user's own login
//   anthropic -> point claude at an Anthropic-compatible base URL
//   openai    -> route claude through the proxy (ccr)
// With { dryRun: true } nothing is spawned or written; returns a description.
export async function launch({ provider, model, apiKey, extraArgs = [], skipPermissions = true, harness = 'claude', dryRun = false }) {
  if (harness === 'omp') {
    return launchOmp({ provider, model, apiKey, extraArgs, skipPermissions, dryRun });
  }

  const claudeArgs = [];
  if (skipPermissions) claudeArgs.push('--dangerously-skip-permissions');
  if (model) claudeArgs.push('--model', provider.mode === 'openai' ? `${provider.id},${model}` : model);
  claudeArgs.push(...extraArgs);

  if (provider.mode === 'openai') {
    if (dryRun) {
      return {
        via: 'proxy (claude-code-router)',
        cmd: which('ccr', globalBinDirs()) || 'ccr',
        args: ['code', ...claudeArgs],
        ccrConfig: CCR_CONFIG,
        route: `${provider.id},${model}`
      };
    }
    writeCcrConfig(provider, model, apiKey);
    const { ccr, dirs } = ensureProxy();
    const env = { ...process.env, NODE_NO_WARNINGS: '1' };
    for (const k of ['ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_API_KEY', 'CLAUDE_CONFIG_DIR', 'CLAUDE_CODE_DISABLE_1M_CONTEXT']) {
      delete env[k];
    }
    env.PATH = [...dirs, env.PATH].join(path.delimiter);
    console.log(`\nLaunching ${provider.name || provider.id} / ${model} via the proxy…`);
    return runInherit(ccr, ['code', ...claudeArgs], env);
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
      baseUrl: provider.mode === 'anthropic' ? provider.baseUrl : '(default)'
    };
  }

  const claude = which('claude');
  if (!claude) throw new Error('The `claude` CLI was not found. Install Claude Code: https://claude.com/claude-code');
  console.log(`\nLaunching ${provider.name || provider.id}${model ? ' / ' + model : ''}…`);
  return runInherit(claude, claudeArgs, env);
}
