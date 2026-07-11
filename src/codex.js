// Codex (ChatGPT subscription) provider — runs Claude Code (or omp) against a
// ChatGPT Codex subscription, with NO dependency on the `codex` CLI.
//
// Choosing it:
//   1) ensures a ChatGPT login (our own PKCE flow in codex-auth.js; reuses an
//      existing codex CLI login if one is present),
//   2) fetches the subscription's live model list and shows bro's picker,
//   3) starts the local Anthropic⇄Codex translation bridge (codex-bridge.js),
//   4) launches the harness pointed at the bridge via ANTHROPIC_BASE_URL, then
//      tears the bridge down when the harness exits.

import path from 'node:path';
import { which, globalBinDirs, runInherit } from './proc.js';
import { select } from './ui.js';
import { rememberSelection, lastModelFor } from './state.js';
import { isCodexLoggedIn, codexLogin, codexLogout, codexAuthStatus } from './codex-auth.js';
import { fetchCodexModels, startCodexBridge, DEFAULT_PORT } from './codex-bridge.js';
import { launchOmp } from './launch.js';

export const CODEX_PROVIDER = {
  id: 'codex',
  name: 'Codex (ChatGPT subscription)',
  mode: 'codex',
  models: []
};

// `bro codex <login|logout|status>` — account management without launching.
export async function runCodexCommand(args = []) {
  const sub = args[0];
  if (sub === 'logout') {
    console.log(codexLogout() ? 'Logged out of the ChatGPT subscription (bro credentials removed).' : 'No bro ChatGPT credentials were stored.');
    return 0;
  }
  if (sub === 'status') {
    const s = codexAuthStatus();
    if (!s.loggedIn) { console.log('Not logged in. Run: bro codex login'); return 0; }
    console.log(`Logged in to ChatGPT${s.plan ? ` (${s.plan} plan)` : ''}.`);
    console.log(`  credentials: ${s.source}`);
    return 0;
  }
  if (sub === 'login' || sub == null) {
    if (sub == null && isCodexLoggedIn()) {
      console.log('Already logged in. Use `bro codex login` to re-authenticate or `bro codex logout` to sign out.');
      return 0;
    }
    try {
      await codexLogin();
      return 0;
    } catch (e) {
      console.error(`Login failed: ${e.message}`);
      return 1;
    }
  }
  console.error(`Unknown codex command: ${sub}  (use: login | logout | status)`);
  return 1;
}

async function ensureLogin() {
  if (isCodexLoggedIn()) return true;
  console.log('\nNo ChatGPT subscription login found.');
  const choice = await select({
    message: 'Log in to ChatGPT to use Codex models?',
    choices: [
      { label: 'Log in now (opens ChatGPT in your browser)', value: 'login' },
      { label: 'Cancel', value: 'cancel' }
    ]
  }).catch(() => ({ value: 'cancel' }));
  if (choice.value !== 'login') return false;
  try {
    await codexLogin();
    return true;
  } catch (e) {
    console.error(`Login failed: ${e.message}`);
    return false;
  }
}

async function chooseModel(models, skip) {
  const lastM = lastModelFor(CODEX_PROVIDER.id);
  const choice = await select({
    message: 'Choose a model for Codex:',
    startIndex: lastM != null ? Math.max(0, models.findIndex((m) => m.id === lastM)) : 0,
    choices: models.map((m) => ({ label: `${m.name}  \x1b[2m(${m.id})\x1b[0m`, value: m.id })),
    toggle: { label: 'Skip permissions', value: skip }
  }).catch(() => null);
  return choice;
}

export async function runCodex({ model = '', harness = 'claude', extraArgs = [], skipPermissions = true, dryRun = false } = {}) {
  if (dryRun) {
    const models = await fetchCodexModels();
    return {
      via: 'codex (chatgpt subscription) → local bridge → ' + (harness === 'omp' ? 'omp' : 'claude'),
      auth: isCodexLoggedIn() ? 'logged in' : 'not logged in (login would run)',
      bridge: `http://127.0.0.1:${DEFAULT_PORT}  (Anthropic-compatible)`,
      model: model || '(menu)',
      models: models.map((m) => m.id),
      harness
    };
  }

  if (!(await ensureLogin())) { console.log('Cancelled.'); return 0; }

  process.stdout.write('\x1b[2mFetching Codex models…\x1b[0m\r');
  const models = await fetchCodexModels();
  process.stdout.write('\x1b[2K\r');

  let skip = skipPermissions;
  // Claude Code always needs a concrete model (it also makes background calls);
  // omp does its own model routing, so only prompt for claude.
  if (!model && harness !== 'omp') {
    const choice = await chooseModel(models, skip);
    if (choice == null) { console.log('Cancelled.'); return 0; }
    model = choice.value;
    if (choice.toggleOn !== undefined) skip = choice.toggleOn;
  }
  const activeModel = model || models[0]?.id || '';

  // Start the local bridge.
  let bridge;
  try {
    bridge = await startCodexBridge({ defaultModel: activeModel, models });
  } catch (e) {
    console.error(`Could not start the Codex bridge: ${e.message}`);
    return 1;
  }

  rememberSelection(CODEX_PROVIDER.id, model, harness);

  const stop = () => bridge.close().catch(() => {});
  try {
    if (harness === 'omp') {
      return await launchOmp({
        provider: {
          ...CODEX_PROVIDER,
          mode: 'anthropic',
          baseUrl: bridge.baseUrl,
          disable1mContext: true,
          models: models.map((m) => ({ id: m.id, name: m.name }))
        },
        model,
        apiKey: 'bro-codex',
        extraArgs,
        skipPermissions: skip,
        dryRun: false
      });
    }

    const claude = which('claude', globalBinDirs());
    if (!claude) throw new Error('The `claude` CLI was not found. Install Claude Code: https://claude.com/claude-code');

    const env = { ...process.env };
    delete env.CLAUDE_CONFIG_DIR;
    delete env.ANTHROPIC_API_KEY;
    env.ANTHROPIC_BASE_URL = bridge.baseUrl;
    env.ANTHROPIC_AUTH_TOKEN = 'bro-codex';
    env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
    env.NODE_NO_WARNINGS = '1';

    const claudeArgs = [];
    if (skip) claudeArgs.push('--dangerously-skip-permissions');
    if (activeModel) claudeArgs.push('--model', activeModel);
    claudeArgs.push(...extraArgs);

    console.log(`\nLaunching Claude Code on your ChatGPT subscription${activeModel ? ' / ' + activeModel : ''}…`);
    console.log(`\x1b[2m  bridge: ${bridge.baseUrl}\x1b[0m\n`);
    return await runInherit(claude, claudeArgs, env);
  } finally {
    stop();
  }
}
