import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { which, globalBinDirs, runInherit, ensureProxy } from './proc.js';

const CCR_CONFIG = path.join(os.homedir(), '.claude-code-router', 'config.json');

// Map a permission posture ('auto' | 'manual' | 'bypass') to the claude CLI flags
// that put it in that mode at startup. Shared by the direct and pool launchers.
export function permissionArgs(mode) {
  if (mode === 'bypass') return ['--dangerously-skip-permissions'];
  if (mode === 'manual') return [];
  return ['--permission-mode', 'auto'];
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
export async function launch({ provider, model, apiKey, extraArgs = [], permissionMode = 'auto', dryRun = false }) {
  const claudeArgs = [...permissionArgs(permissionMode)];
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
      cmd: which('claude') || 'claude',
      args: claudeArgs,
      baseUrl: provider.mode === 'anthropic' ? provider.baseUrl : '(default)'
    };
  }

  const claude = which('claude');
  if (!claude) throw new Error('The `claude` CLI was not found. Install Claude Code: https://claude.com/claude-code');
  console.log(`\nLaunching ${provider.name || provider.id}${model ? ' / ' + model : ''}…`);
  return runInherit(claude, claudeArgs, env);
}
