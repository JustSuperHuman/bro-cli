// "Multiple Claude Account Proxy" — the top bro option.
//
// Pools any number of Claude Max / Team logins behind one local Anthropic-
// compatible endpoint and launches Claude Code against it, so a single session
// draws from several plans and fails over automatically when one runs out.
//
// The pool server itself lives in ../pool (a Bun/TypeScript app). This module is
// the Node-side orchestrator: it ensures at least one account is authenticated,
// starts the pool server in the background, waits for it to become healthy, then
// runs `claude` in the foreground pointed at it — tearing the server down when
// Claude exits.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { which, globalBinDirs, runInherit, ensureBun, ensureClaude } from './proc.js';
import { select, selectColumns, prompt, holdOrContinue } from './ui.js';
import { launchOmp, launchPi, permissionArgs } from './launch.js';
import { describeJev, ensureJev, jevCommandPath, jevNotice } from './jev.js';
import { launchDsh } from './deepseek.js';
import { note } from './out.js';
import { fetchClaudeUsage, usageSummary } from './claude-usage.js';
import { listSessions } from './sessions.js';
import { chooseResumeProfile, sessionRows, stageFiles } from './profiles.js';
import { claudeBrowserEnabled, prepareClaudeBrowser } from './claude-browser.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const POOL_ROOT = path.join(__dirname, '..', 'pool');
const POOL_ENTRY = path.join(POOL_ROOT, 'src', 'index.ts');

const DEFAULT_PORT = 3456;
const POOL_DIR = process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
const ACCOUNTS_DIR = path.join(POOL_DIR, 'accounts');
// Claude Code's own config dir — the login used when no profile is selected.
const DEFAULT_CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
const PROXY_LOG = path.join(os.homedir(), '.bro', 'pool-proxy.log');

// No model list here: bro never picks a model for the pool. Claude Code and
// omp have their own model pickers; Pi receives the pool's live model list and
// starts on its first concrete model. -m still forces one for every harness.
export const POOL_PROVIDER = {
  id: 'pool',
  name: 'Multiple Claude Account Proxy',
  mode: 'pool',
  models: []
};

export const ACCOUNT_PROVIDER = {
  id: 'account',
  name: 'Claude Account Profile',
  mode: 'account',
  models: []
};

// Written to omp's models.yml only when the live /v1/models fetch fails.
const FALLBACK_MODELS = [
  { id: 'claude-fable-5', name: 'Claude Fable 5' },
  { id: 'claude-sonnet-5', name: 'Claude Sonnet 5' },
  { id: 'claude-opus-4-8', name: 'Claude Opus 4.8' },
  { id: 'claude-haiku-4-5-20251001', name: 'Claude Haiku 4.5' }
];

// --- account inspection (read the pool's on-disk state directly) -----------

function listAccounts() {
  let names = [];
  try {
    names = fs
      .readdirSync(ACCOUNTS_DIR, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name)
      .sort();
  } catch {
    return [];
  }
  return names.map((name) => {
    let authenticated = false;
    let subscriptionType = null;
    try {
      const creds = JSON.parse(fs.readFileSync(path.join(ACCOUNTS_DIR, name, '.credentials.json'), 'utf8'));
      const oauth = creds && creds.claudeAiOauth;
      authenticated = Boolean(oauth && oauth.accessToken);
      subscriptionType = (oauth && oauth.subscriptionType) || null;
    } catch {
      /* no creds yet */
    }
    return { name, authenticated, subscriptionType };
  });
}

function accountDirFor(name) {
  return path.join(ACCOUNTS_DIR, name);
}

const samePath = (a, b) => {
  if (!a || !b) return false;
  const left = path.resolve(a);
  const right = path.resolve(b);
  return process.platform === 'win32' ? left.toLowerCase() === right.toLowerCase() : left === right;
};

const configDirForAccount = (name) => name ? accountDirFor(name) : DEFAULT_CLAUDE_DIR;

// Usage percentages carry their own color by pressure (green → amber → red)
// so the stats read at a glance instead of being one dim blur.
function usagePercent(value) {
  if (typeof value !== 'number') return '\x1b[2m—\x1b[0m';
  const pct = Math.round(value);
  const color = pct >= 80 ? '\x1b[31m' : pct >= 50 ? '\x1b[33m' : '\x1b[32m';
  return `${color}${pct}%\x1b[0m`;
}

export { usageSummary };

export function accountLabel(a) {
  const state = a.authenticated ? 'ready' : 'logged out';
  const plan = a.subscriptionType ? ` \x1b[2m· ${a.subscriptionType}\x1b[0m` : '';
  if (a.authenticated && a.usageStats) {
    const u = a.usageStats;
    const usage = `\x1b[2m5h\x1b[0m ${usagePercent(u.session)} \x1b[2m· wk\x1b[0m ${usagePercent(u.weekly)} \x1b[2m· Fable\x1b[0m ${usagePercent(u.fable)}`;
    return `${a.name}  ${usage}${plan}`;
  }
  if (a.authenticated && a.usageStats === null) return `${a.name}  \x1b[2musage unavailable\x1b[0m${plan}`;
  return `${a.name}  \x1b[2m${state}\x1b[0m${plan}`;
}

async function fetchAccountUsage(account) {
  return fetchClaudeUsage({ configDir: accountDirFor(account.name) });
}

async function loadAccountUsages(accounts) {
  return Promise.all(
    accounts.map(async (account) => {
      if (!account.authenticated) return account;
      try {
        return { ...account, usageStats: await fetchAccountUsage(account) };
      } catch {
        return { ...account, usageStats: null };
      }
    })
  );
}

// Account list for the two-column picker's right pane: one entry per profile
// with live usage stats in the label, plus a manage entry (empty value) that
// falls through to the full account menu.
//
// Below the profiles come the sessions those profiles can resume — this
// project's first, then everything else with its path, all reachable by typing
// to filter. A session row carries its source profile; after selection, the
// user can keep that owner or choose a different profile to resume with.
export async function accountProfileChoices() {
  let accounts = listAccounts();
  if (accounts.some((a) => a.authenticated)) accounts = await loadAccountUsages(accounts);

  const rows = [
    ...accounts.map((a) => ({ label: accountLabel(a), value: a.name })),
    { label: 'Log in / manage accounts…', value: '' }
  ];

  let sessions = [];
  try {
    sessions = await listSessions({
      sources: [
        // The machine's own Claude login owns sessions too. Cross-profile
        // resume stages a fork when the chosen destination differs.
        { account: null, configDir: DEFAULT_CLAUDE_DIR },
        ...accounts
          .filter((a) => a.authenticated)
          .map((a) => ({ account: a.name, configDir: accountDirFor(a.name) }))
      ]
    });
  } catch {
    // Session history is a convenience — never let it cost you the account menu.
    return rows;
  }

  return [...rows, ...sessionRows(sessions, (s) => ({
    kind: 'session',
    id: s.id,
    account: s.account,
    cwd: s.cwd,
    title: s.title,
    file: s.file
  }))];
}

// The direct `bro account` route used to open the older profile-only selector.
// Keep one provider row on the left so the direct `bro account` route uses the
// same combined profile/session list as the main provider picker.
async function chooseAccountTarget() {
  const choice = await selectColumns({
    message: 'Choose a Claude account profile or session:',
    choices: [{
      label: ACCOUNT_PROVIDER.name,
      detail: 'pick login',
      children: accountProfileChoices,
      filterableChildren: true
    }]
  }).catch(() => null);

  if (!choice) return null;
  const value = choice.child?.value;
  if (value && typeof value === 'object' && value.kind === 'session') {
    return { accountName: '', session: value };
  }
  return { accountName: typeof value === 'string' ? value : '', session: null };
}

async function chooseAccountProfile(preferredName) {
  let bun = null;
  const needBun = () => {
    bun ||= findBun();
    return bun;
  };

  while (true) {
    let accounts = listAccounts();
    if (preferredName) {
      const found = accounts.find((a) => a.name === preferredName);
      if (!found) throw new Error(`Unknown account profile: ${preferredName}. Run "bro accounts list" to see profiles.`);
      if (found.authenticated) return found;
      console.log(`Account "${preferredName}" exists but is not logged in yet.`);
      await runPoolCli(needBun(), ['accounts', 'login', preferredName]);
      preferredName = null;
      continue;
    }

    if (accounts.some((account) => account.authenticated)) {
      process.stderr.write('\x1b[2mLoading profile usage…\x1b[0m\r');
      accounts = await loadAccountUsages(accounts);
      process.stderr.write('\x1b[2K\r');
    }

    const choices = [
      ...accounts.map((a) => ({
        label: accountLabel(a),
        value: a.authenticated ? { action: 'use', account: a } : { action: 'login', name: a.name }
      })),
      { label: 'Log in / add another Claude account', value: { action: 'login' } },
      { label: "Import this machine's current Claude login", value: { action: 'import' } },
      { label: 'Cancel', value: { action: 'cancel' } }
    ];

    const choice = await select({
      message: 'Choose a Claude account profile:',
      choices
    }).catch(() => ({ value: { action: 'cancel' } }));

    const picked = choice.value;
    if (picked.action === 'cancel') return null;
    if (picked.action === 'use') return picked.account;
    if (picked.action === 'login') {
      const fallback = picked.name || 'work';
      const name = picked.name || (await prompt(`Account name [${fallback}]: `).catch(() => '')) || fallback;
      await runPoolCli(needBun(), ['accounts', 'login', name]);
    } else if (picked.action === 'import') {
      const name = (await prompt('Name for the imported account [primary]: ').catch(() => '')) || 'primary';
      await runPoolCli(needBun(), ['accounts', 'import', name]);
    }
  }
}

// A session defaults to the login that owns its transcript, but every stored
// profile (plus the machine's native login) is available as a destination.
// Choosing another destination causes runAccountProfile to stage a fork.
async function chooseAccountForResume(session) {
  let accounts = listAccounts();
  if (accounts.some((account) => account.authenticated)) accounts = await loadAccountUsages(accounts);

  const target = await chooseResumeProfile({
    session,
    profiles: accounts.map((account) => ({ name: account.name, label: accountLabel(account) })),
    localLabel: "This machine's Claude login",
    manageLabel: 'Log in / manage accounts…'
  });
  if (!target) return null;
  if (!target.manage) return { local: target.local, accountName: target.name };
  const account = await chooseAccountProfile();
  return account ? { local: false, accountName: account.name } : null;
}

const SESSION_AUX_DIRS = ['file-history', 'session-env', 'shell-snapshots', 'tasks'];

// Temporarily make a source transcript visible inside another profile. Claude's
// --fork-session reads this copy and writes the continued conversation under a
// new id; cleanup removes only the artifacts created here, never the source or
// the fork. Exported so the file-safety behavior can be covered in isolation.
export function stageSessionForProfile(session, { sourceConfigDir, targetConfigDir }) {
  const id = String(session?.id || '');
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
    throw new Error(`Invalid Claude session id: ${id || '(missing)'}`);
  }
  if (!session?.file) throw new Error('That session has no transcript path. Reopen the switcher and try again.');

  const sourceRoot = path.resolve(sourceConfigDir, 'projects');
  const sourceFile = path.resolve(session.file);
  const sourceProjectDir = path.dirname(sourceFile);
  if (
    path.basename(sourceFile) !== `${id}.jsonl` ||
    !samePath(path.dirname(sourceProjectDir), sourceRoot) ||
    !fs.existsSync(sourceFile)
  ) {
    throw new Error(`The source transcript is unavailable or outside its Claude profile: ${sourceFile}`);
  }

  const targetRoot = path.resolve(targetConfigDir);
  const targetProjectDir = path.join(targetRoot, 'projects', path.basename(sourceProjectDir));
  const targetFile = path.join(targetProjectDir, `${id}.jsonl`);

  // A Claude session is the transcript plus the per-session artifacts Claude
  // keeps beside it: the project's sidecar directory and the aux directories
  // keyed by session id.
  const { cleanup } = stageFiles({
    targetRoot,
    entries: [
      { source: sourceFile, target: targetFile },
      { source: path.join(sourceProjectDir, id), target: path.join(targetProjectDir, id) },
      ...SESSION_AUX_DIRS.map((name) => ({
        source: path.join(sourceConfigDir, name, id),
        target: path.join(targetRoot, name, id)
      }))
    ]
  });
  return { targetFile, cleanup };
}

// --- bun discovery ---------------------------------------------------------

function findBun() {
  return ensureBun().bun;
}

// Run a pool CLI sub-command (`accounts …`) with inherited stdio so interactive
// logins work. Resolves with the child's exit code.
function runPoolCli(bun, args) {
  return runInherit(bun, ['run', POOL_ENTRY, ...args], { ...process.env, CLAUDE_POOL_DIR: POOL_DIR });
}

export function runPoolAccounts(args = []) {
  return runPoolCli(findBun(), ['accounts', ...args]);
}

// --- account setup flow ----------------------------------------------------

async function ensureAccount(bun) {
  while (true) {
    const accounts = listAccounts();
    const authed = accounts.filter((a) => a.authenticated);
    if (authed.length > 0) return authed;

    console.log('\nNo authenticated Claude accounts in the pool yet.');
    const choice = await select({
      message: 'Add your first account:',
      choices: [
        { label: 'Log in a new Claude account (opens Claude to sign in)', value: 'login' },
        { label: "Import this machine's existing Claude login", value: 'import' },
        { label: 'Cancel', value: 'cancel' }
      ]
    }).catch(() => ({ value: 'cancel' }));

    if (choice.value === 'cancel') return [];

    if (choice.value === 'import') {
      const name = (await prompt('Name for the imported account [primary]: ').catch(() => '')) || 'primary';
      await runPoolCli(bun, ['accounts', 'import', name]);
    } else {
      const name = (await prompt('Name for the new account [work]: ').catch(() => '')) || 'work';
      console.log(`\nOpening Claude to sign in as "${name}". Run /login, finish sign-in, then /exit.\n`);
      await runPoolCli(bun, ['accounts', 'login', name]);
    }
    // Loop re-checks; user can add more or proceed once at least one is authed.
  }
}

// --- proxy server lifecycle ------------------------------------------------

async function healthy(port) {
  try {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 1500);
    const res = await fetch(`http://127.0.0.1:${port}/health`, {
      signal: ctrl.signal,
      headers: { connection: 'close' }
    });
    clearTimeout(t);
    return res.ok;
  } catch {
    return false;
  }
}

async function waitHealthy(port, timeoutMs = 15000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (await healthy(port)) return true;
    await new Promise((r) => setTimeout(r, 300));
  }
  return false;
}

function startProxy(bun, port) {
  fs.mkdirSync(path.dirname(PROXY_LOG), { recursive: true });
  const out = fs.openSync(PROXY_LOG, 'a');
  const child = spawn(bun, ['run', POOL_ENTRY, 'serve'], {
    env: { ...process.env, CLAUDE_POOL_DIR: POOL_DIR, PORT: String(port), HOST: '127.0.0.1' },
    stdio: ['ignore', out, out],
    windowsHide: true
  });
  child.unref?.();
  return child;
}

// --- status panel ----------------------------------------------------------

const C = {
  dim: (s) => `\x1b[2m${s}\x1b[0m`,
  bold: (s) => `\x1b[1m${s}\x1b[0m`,
  green: (s) => `\x1b[32m${s}\x1b[0m`,
  amber: (s) => `\x1b[33m${s}\x1b[0m`,
  red: (s) => `\x1b[31m${s}\x1b[0m`
};

async function fetchStatus(port) {
  try {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 2000);
    const res = await fetch(`http://127.0.0.1:${port}/api/status`, {
      signal: ctrl.signal,
      headers: { connection: 'close' }
    });
    clearTimeout(t);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

function fmtTokens(n) {
  n = n || 0;
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}

function printStatus(status, baseUrl) {
  const accounts = (status && status.accounts) || [];
  const avail = accounts.filter((a) => a.available).length;
  const nameW = Math.max(4, ...accounts.map((a) => a.name.length));
  const planW = Math.max(4, ...accounts.map((a) => (a.subscriptionType || '?').length));

  note('');
  note('  ' + C.bold('Multiple Claude Account Proxy') + C.dim(`  —  ${avail} of ${accounts.length} ready`));
  note('');
  for (const a of accounts) {
    const dot = a.available ? C.green('●') : a.authenticated ? C.amber('●') : C.red('●');
    const u = a.usage || {};
    const tok = fmtTokens((u.windowInputTokens || 0) + (u.windowOutputTokens || 0));
    const usage = C.dim(`${(u.windowRequests || 0)} req · ${tok} tok`);
    const tier = C.dim(a.rateLimitTier || '-');
    const state = a.available ? '' : '  ' + C.amber(a.unavailableReason || 'unavailable');
    note(
      `  ${dot} ${a.name.padEnd(nameW)}  ${(a.subscriptionType || '?').padEnd(planW)}  ${tier}   ${usage}${state}`
    );
  }
  note('');
  note('  ' + C.dim('Dashboard ') + `${baseUrl}/`);
  note('  ' + C.dim('Endpoint  ') + `${baseUrl}` + C.dim('  (Anthropic-compatible · pooled)'));
  note('');
}

// --- entry point -----------------------------------------------------------

// Ask the running pool server for the live Anthropic model list (it proxies
// /v1/models upstream with a pooled OAuth token). Null on any failure.
async function fetchPoolModels(port) {
  try {
    const ctrl = new AbortController();
    const t = setTimeout(() => ctrl.abort(), 10000);
    const res = await fetch(`http://127.0.0.1:${port}/v1/models`, {
      signal: ctrl.signal,
      headers: { connection: 'close' }
    });
    clearTimeout(t);
    if (!res.ok) return null;
    const json = await res.json();
    const models = (json.data || [])
      .filter((m) => m && m.id)
      .map((m) => ({ id: m.id, name: m.display_name || m.id }));
    return models.length ? models : null;
  } catch {
    return null;
  }
}

function poolHarnessProvider(baseUrl, models) {
  return {
    ...POOL_PROVIDER,
    mode: 'anthropic',
    baseUrl,
    disable1mContext: true,
    // omp requests go straight through to the Anthropic API, which 404s on
    // aliases like "opus" — only real model ids may reach models.yml.
    models: models && models.length ? models : FALLBACK_MODELS
  };
}

// `session` (from the combined picker) runs Claude in that session's directory.
// Its owner is the default destination; choosing another profile stages the
// transcript there and uses Claude's --fork-session so neither copy overwrites
// the original conversation.
export async function runAccountProfile({
  accountName = '',
  model = '',
  extraArgs = [],
  permissionMode,
  skipPermissions = permissionMode ? permissionMode === 'bypass' : true,
  session = null,
  resumeWithLocal = false,
  // An account profile is a claude.ai login like any other, so Jev Router can
  // front it: jev-claude inherits this profile's CLAUDE_CONFIG_DIR unchanged.
  jev = false,
  headless = false,
  dryRun = false
} = {}) {
  const accounts = listAccounts();

  if (dryRun) {
    const targetName = accountName || session?.account || '';
    const local = Boolean(session && (resumeWithLocal || (!accountName && !session.account)));
    const sourceConfigDir = session ? configDirForAccount(session.account) : null;
    const targetConfigDir = session
      ? (local ? DEFAULT_CLAUDE_DIR : configDirForAccount(targetName))
      : (targetName ? configDirForAccount(targetName) : null);
    const crossProfile = Boolean(session && !samePath(sourceConfigDir, targetConfigDir));
    const browserEnabled = claudeBrowserEnabled()
      && !extraArgs.includes('--no-chrome')
      && !extraArgs.includes('--chrome');
    const claudePath = which('claude', globalBinDirs()) || 'claude';
    const dryEnv = { ...process.env };
    if (targetConfigDir) dryEnv.CLAUDE_CONFIG_DIR = targetConfigDir;
    else if (local) delete dryEnv.CLAUDE_CONFIG_DIR;
    const browser = browserEnabled
      ? await prepareClaudeBrowser({ claudePath, baseEnv: dryEnv, skipPermissions, autoStart: false })
      : null;
    const resumeArgs = session?.id
      ? ['--resume', session.id, ...(crossProfile ? ['--fork-session'] : [])]
      : [];
    return {
      via: `${crossProfile ? 'claude cross-profile session fork' : local ? 'claude local login' : 'claude account profile'}${jev ? ' via jev-router' : ''}`,
      ...(jev ? { jev: describeJev('claude') } : {}),
      poolDir: POOL_DIR,
      account: local ? '(this machine)' : targetName || '(menu)',
      accounts,
      ...(session ? {
        resume: session.id,
        cwd: session.cwd,
        sourceAccount: session.account || '(this machine)',
        forkSession: crossProfile
      } : {}),
      claude: {
        cmd: jev ? jevCommandPath('jev-claude') : claudePath,
        args: [...permissionArgs(permissionMode || (skipPermissions ? 'bypass' : 'manual')), ...(browser?.args || []), ...(model && !jev ? ['--model', model] : []), ...resumeArgs, ...extraArgs],
        env: {
          ...(local
            ? { CLAUDE_CONFIG_DIR: process.env.CLAUDE_CONFIG_DIR || '(unset — this machine\'s login)' }
            : targetConfigDir ? { CLAUDE_CONFIG_DIR: targetConfigDir } : { CLAUDE_CONFIG_DIR: '(selected account profile)' }),
          ...(browser ? { CLAUDE_CODE_ENABLE_CFC: browser.env.CLAUDE_CODE_ENABLE_CFC } : {})
        }
      }
    };
  }

  // `bro account` with no name opens the same combined profile/session list as
  // the main provider picker.
  if (!accountName && !session) {
    const target = await chooseAccountTarget();
    if (!target) {
      console.log('Cancelled.');
      return 0;
    }
    accountName = target.accountName;
    session = target.session;
  }

  let local = false;
  if (session) {
    if (resumeWithLocal) {
      local = true;
      accountName = '';
    } else if (!accountName) {
      const target = await chooseAccountForResume(session);
      if (!target) {
        console.log('Cancelled.');
        return 0;
      }
      local = target.local;
      accountName = target.accountName;
    }
  }

  const account = local ? { name: 'this machine' } : await chooseAccountProfile(accountName);
  if (!account) {
    console.log('Cancelled.');
    return 0;
  }

  const { claude, dirs } = ensureClaude();
  // jev-claude runs the real `claude` from PATH, so Claude Code is resolved
  // first and its directory is in the environment the wrapper inherits.
  const jevClaude = jev ? ensureJev('claude') : null;

  const sourceConfigDir = session ? configDirForAccount(session.account) : null;
  const targetConfigDir = local ? DEFAULT_CLAUDE_DIR : accountDirFor(account.name);
  const crossProfile = Boolean(session && !samePath(sourceConfigDir, targetConfigDir));
  const resumeArgs = session?.id
    ? ['--resume', session.id, ...(crossProfile ? ['--fork-session'] : [])]
    : [];
  const cwd = session?.cwd && session.cwd !== process.cwd() ? session.cwd : undefined;

  if (cwd && !fs.existsSync(cwd)) {
    console.error(`\nThat session's directory is gone: ${cwd}`);
    return 1;
  }

  const env = { ...process.env };
  if (local) {
    if (process.env.CLAUDE_CONFIG_DIR) env.CLAUDE_CONFIG_DIR = DEFAULT_CLAUDE_DIR;
    else delete env.CLAUDE_CONFIG_DIR;
  } else {
    env.CLAUDE_CONFIG_DIR = targetConfigDir;
  }
  for (const k of ['ANTHROPIC_BASE_URL', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_API_KEY', 'CLAUDE_CODE_DISABLE_1M_CONTEXT']) {
    delete env[k];
  }
  env.NODE_NO_WARNINGS = '1';
  env.PATH = [...(jevClaude?.dirs || []), ...dirs, env.PATH || ''].join(path.delimiter);

  // An account profile is a claude.ai login, but only the login the browser
  // extension is signed into (the "owner") can use Claude Code's own --chrome
  // wiring — the bridge answers other accounts with an empty browser list.
  // prepareClaudeBrowser detects that and hands foreign logins the browser
  // server running as the owner instead.
  const browser = extraArgs.includes('--no-chrome') || extraArgs.includes('--chrome')
    ? null
    : await prepareClaudeBrowser({ claudePath: claude, baseEnv: env, skipPermissions, autoStart: !headless });
  if (browser) Object.assign(env, browser.env);

  const claudeArgs = permissionArgs(permissionMode || (skipPermissions ? 'bypass' : 'manual'));
  claudeArgs.push(...(browser?.args || []));
  // With Jev Router in front, the model is chosen per turn — pinning one here
  // would pause routing for the whole session.
  if (model && !jevClaude) claudeArgs.push('--model', model);
  claudeArgs.push(...resumeArgs, ...extraArgs);

  const title = String(session?.title || session?.id || '');
  const shortTitle = title.length > 60 ? title.slice(0, 59) + '…' : title;
  const sourceName = session?.account || 'this machine';
  const banner = session
    ? crossProfile
      ? `Forking “${shortTitle}” from ${sourceName} and resuming as ${account.name}`
      : `Resuming “${shortTitle}” as ${account.name}`
    : `Launching Claude Code as ${account.name}`;
  note(`\n${banner}${model && !jevClaude ? ' / ' + model : ''}${cwd ? `\nin ${cwd}` : ''}...\n`);
  if (jevClaude) note(jevNotice('claude'));

  const staged = crossProfile
    ? stageSessionForProfile(session, { sourceConfigDir, targetConfigDir })
    : null;
  try {
    return await runInherit(jevClaude?.executable || claude, claudeArgs, env, { cwd, terminalAgent: 'claude' });
  } finally {
    staged?.cleanup();
  }
}

export async function runPool({
  model = '',
  extraArgs = [],
  permissionMode,
  skipPermissions = permissionMode ? permissionMode === 'bypass' : true,
  harness = 'claude',
  providers = [],
  providerKeys = {},
  headless = false,
  dryRun = false
} = {}) {
  // The pool endpoint is Anthropic-shaped — codex only speaks OpenAI's wire
  // formats, so there is nothing to point it at here.
  if (harness === 'codex') {
    console.error('The codex harness can\'t use the account pool: the pool serves the Anthropic API, and codex only talks to OpenAI-compatible endpoints.');
    console.error('  Use claude, omp, Pi, or DeepSeek Harness for the pool (press h in the picker, or pass --claude / --omp / --pi / --dsh).');
    return 1;
  }

  const port = Number.parseInt(process.env.PORT || '', 10) || DEFAULT_PORT;
  const baseUrl = `http://127.0.0.1:${port}`;
  const apiKey = process.env.PROXY_API_KEY || 'claude-max-pool';

  if (dryRun) {
    const out = {
      via: 'multiple-account pool',
      poolServer: `bun run ${POOL_ENTRY} serve`,
      poolDir: POOL_DIR,
      backend: process.env.CLAUDE_POOL_BACKEND || 'oauth',
      baseUrl,
      accounts: listAccounts(),
      harness
    };
    if (harness === 'omp') {
      out.omp = await launchOmp({
        provider: poolHarnessProvider(baseUrl),
        model,
        apiKey,
        extraArgs,
        skipPermissions,
        dryRun: true
      });
    } else if (harness === 'pi') {
      out.pi = await launchPi({
        provider: poolHarnessProvider(baseUrl),
        model,
        apiKey,
        extraArgs,
        dryRun: true
      });
    } else if (harness === 'dsh') {
      const bridgeProvider = poolHarnessProvider(baseUrl);
      out.dsh = await launchDsh({
        provider: bridgeProvider,
        model,
        apiKey,
        providers: [bridgeProvider, ...providers],
        providerKeys: { ...providerKeys, [bridgeProvider.id]: apiKey },
        extraArgs,
        skipPermissions,
        dryRun: true
      });
    } else {
      out.claude = {
        cmd: which('claude', globalBinDirs()) || 'claude',
        args: [...permissionArgs(permissionMode || (skipPermissions ? 'bypass' : 'manual')), ...(model ? ['--model', model] : []), ...extraArgs],
        env: { ANTHROPIC_BASE_URL: baseUrl }
      };
    }
    return out;
  }

  const bun = findBun();

  // 1) Make sure we have at least one authenticated account.
  const authed = await ensureAccount(bun);
  if (authed.length === 0) {
    console.log('No accounts configured — nothing to launch.');
    return 0;
  }
  // 2) Start the proxy (reuse an already-running one on this port).
  // The server is a Bun process, and Bun has been seen to segfault mid-session
  // on Windows — the harness then reports "The socket connection was closed
  // unexpectedly". Supervise the child and restart it so a crash costs one
  // failed request instead of the rest of the session.
  let proxyChild = null;
  let stopping = false;
  let fastExits = 0;
  const spawnSupervised = () => {
    const startedAt = Date.now();
    proxyChild = startProxy(bun, port);
    proxyChild.on('error', (e) => console.error(`Pool server error: ${e.message}`));
    proxyChild.on('exit', (code, signal) => {
      if (stopping) return;
      fastExits = Date.now() - startedAt < 5000 ? fastExits + 1 : 0;
      if (fastExits >= 3) {
        console.error(`\n  ⚠ The pool server keeps crashing — not restarting it again. Log: ${PROXY_LOG}`);
        proxyChild = null;
        return;
      }
      console.error(`\n  ⚠ The pool server died (${signal || `exit code ${code}`}) — restarting it. Log: ${PROXY_LOG}`);
      const t = setTimeout(() => {
        if (!stopping) spawnSupervised();
      }, 500);
      t.unref?.();
    });
  };

  const already = await healthy(port);
  if (!already) {
    note(`\nStarting the pool server on ${baseUrl} …`);
    spawnSupervised();
    const ok = await waitHealthy(port);
    if (!ok) {
      stopping = true;
      try {
        proxyChild?.kill();
      } catch {}
      throw new Error(
        `The pool server did not become healthy on ${baseUrl}.\n` + `  Check the log: ${PROXY_LOG}`
      );
    }
  }

  const stopProxy = () => {
    stopping = true;
    if (proxyChild) {
      try {
        proxyChild.kill();
      } catch {}
    }
  };

  // 3) Flash the live status, then launch. Hold ~1.5s; enter launches now,
  //    any other key pauses so you can read it, esc cancels.
  const status = await fetchStatus(port);
  printStatus(status, baseUrl);
  const harnessLabel = harness === 'omp' ? 'omp' : harness === 'pi' ? 'Pi' : harness === 'dsh' ? 'DeepSeek Harness' : 'Claude';
  // A headless run has no one to hold for, and its stdout belongs to the
  // harness — so neither the pause nor the line offering it happens there.
  let go = true;
  if (!headless) {
    process.stderr.write('  ' + C.dim(`Launching ${harnessLabel}…  `) + C.dim('enter = now · any key = pause · esc = cancel'));
    go = await holdOrContinue({ ms: 1500 });
    process.stderr.write('\n');
  }
  if (!go) {
    stopProxy();
    console.log('Cancelled.');
    return 0;
  }

  if (harness === 'omp') {
    try {
      const liveModels = await fetchPoolModels(port);
      return await launchOmp({
        provider: poolHarnessProvider(baseUrl, liveModels),
        model,
        apiKey,
        extraArgs,
        skipPermissions,
        dryRun: false
      });
    } finally {
      stopProxy();
    }
  }
  if (harness === 'pi') {
    try {
      const liveModels = await fetchPoolModels(port);
      return await launchPi({
        provider: poolHarnessProvider(baseUrl, liveModels),
        model,
        apiKey,
        extraArgs,
        dryRun: false
      });
    } finally {
      stopProxy();
    }
  }
  if (harness === 'dsh') {
    try {
      const liveModels = await fetchPoolModels(port);
      const bridgeProvider = poolHarnessProvider(baseUrl, liveModels);
      return await launchDsh({
        provider: bridgeProvider,
        model,
        apiKey,
        providers: [bridgeProvider, ...providers],
        providerKeys: { ...providerKeys, [bridgeProvider.id]: apiKey },
        extraArgs,
        skipPermissions,
        dryRun: false
      });
    } finally {
      stopProxy();
    }
  }

  // 4) Launch Claude Code pointed at the pool. Claude speaks the Anthropic API;
  //    the pool serves /v1/messages and routes across account OAuth tokens.
  let claude;
  let claudeDirs;
  try {
    ({ claude, dirs: claudeDirs } = ensureClaude());
  } catch (error) {
    stopProxy();
    throw error;
  }

  const env = { ...process.env };
  delete env.CLAUDE_CONFIG_DIR; // use the user's normal Claude Code workspace/config
  delete env.ANTHROPIC_API_KEY;
  delete env.CLAUDE_CODE_DISABLE_1M_CONTEXT;
  env.ANTHROPIC_BASE_URL = baseUrl;
  env.ANTHROPIC_AUTH_TOKEN = process.env.PROXY_API_KEY || 'claude-max-pool';
  env.NODE_NO_WARNINGS = '1';
  env.PATH = [...claudeDirs, env.PATH || ''].join(path.delimiter);

  // The pool serves Claude over a local ANTHROPIC_BASE_URL with its own token,
  // which is exactly the auth shape that switches Claude Code's own browser
  // wiring off — so these sessions reach the browser through the MCP server.
  const browser = extraArgs.includes('--no-chrome') || extraArgs.includes('--chrome')
    ? null
    : await prepareClaudeBrowser({ claudePath: claude, baseEnv: env, skipPermissions, autoStart: !headless });
  if (browser) Object.assign(env, browser.env);

  const claudeArgs = permissionArgs(permissionMode || (skipPermissions ? 'bypass' : 'manual'));
  claudeArgs.push(...(browser?.args || []));
  if (model) claudeArgs.push('--model', model);
  claudeArgs.push(...extraArgs);

  note(`Launching Claude Code through the account pool${model ? ' / ' + model : ''}…\n`);
  try {
    return await runInherit(claude, claudeArgs, env, { terminalAgent: 'claude' });
  } finally {
    stopProxy();
  }
}
