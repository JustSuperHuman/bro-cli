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
import { which, globalBinDirs, runInherit } from './proc.js';
import { permissionArgs } from './launch.js';
import { select, prompt, holdOrContinue } from './ui.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const POOL_ROOT = path.join(__dirname, '..', 'pool');
const POOL_ENTRY = path.join(POOL_ROOT, 'src', 'index.ts');

const DEFAULT_PORT = 3456;
const POOL_DIR = process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
const ACCOUNTS_DIR = path.join(POOL_DIR, 'accounts');
const PROXY_LOG = path.join(os.homedir(), '.bro', 'pool-proxy.log');

export const POOL_PROVIDER = {
  id: 'pool',
  name: 'Multiple Claude Account Proxy',
  mode: 'pool'
};

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

// --- bun discovery ---------------------------------------------------------

function findBun() {
  const bun = which('bun', globalBinDirs());
  if (!bun) {
    throw new Error(
      'This feature needs Bun to run the pool server.\n' +
        '  Install it: https://bun.sh  (curl -fsSL https://bun.sh/install | bash)\n' +
        '  or:  npm install -g bun'
    );
  }
  return bun;
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

  console.log('');
  console.log('  ' + C.bold('Multiple Claude Account Proxy') + C.dim(`  —  ${avail} of ${accounts.length} ready`));
  console.log('');
  for (const a of accounts) {
    const dot = a.available ? C.green('●') : a.authenticated ? C.amber('●') : C.red('●');
    const u = a.usage || {};
    const tok = fmtTokens((u.windowInputTokens || 0) + (u.windowOutputTokens || 0));
    const usage = C.dim(`${(u.windowRequests || 0)} req · ${tok} tok`);
    const tier = C.dim(a.rateLimitTier || '-');
    const state = a.available ? '' : '  ' + C.amber(a.unavailableReason || 'unavailable');
    console.log(
      `  ${dot} ${a.name.padEnd(nameW)}  ${(a.subscriptionType || '?').padEnd(planW)}  ${tier}   ${usage}${state}`
    );
  }
  console.log('');
  console.log('  ' + C.dim('Dashboard ') + `${baseUrl}/`);
  console.log('  ' + C.dim('Endpoint  ') + `${baseUrl}` + C.dim('  (Anthropic-compatible · pooled)'));
  console.log('');
}

// --- entry point -----------------------------------------------------------

export async function runPool({ extraArgs = [], permissionMode = 'auto', dryRun = false } = {}) {
  const port = Number.parseInt(process.env.PORT || '', 10) || DEFAULT_PORT;
  const baseUrl = `http://127.0.0.1:${port}`;

  if (dryRun) {
    return {
      via: 'multiple-account pool',
      poolServer: `bun run ${POOL_ENTRY} serve`,
      poolDir: POOL_DIR,
      backend: process.env.CLAUDE_POOL_BACKEND || 'oauth',
      baseUrl,
      accounts: listAccounts(),
      claude: {
        cmd: which('claude') || 'claude',
        args: [...permissionArgs(permissionMode), ...extraArgs],
        env: { ANTHROPIC_BASE_URL: baseUrl }
      }
    };
  }

  const bun = findBun();

  // 1) Make sure we have at least one authenticated account.
  const authed = await ensureAccount(bun);
  if (authed.length === 0) {
    console.log('No accounts configured — nothing to launch.');
    return 0;
  }
  // 2) Start the proxy (reuse an already-running one on this port).
  let proxyChild = null;
  const already = await healthy(port);
  if (!already) {
    console.log(`\nStarting the pool server on ${baseUrl} …`);
    proxyChild = startProxy(bun, port);
    proxyChild.on('error', (e) => console.error(`Pool server error: ${e.message}`));
    const ok = await waitHealthy(port);
    if (!ok) {
      try {
        proxyChild.kill();
      } catch {}
      throw new Error(
        `The pool server did not become healthy on ${baseUrl}.\n` + `  Check the log: ${PROXY_LOG}`
      );
    }
  }

  const stopProxy = () => {
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
  process.stdout.write('  ' + C.dim('Launching Claude…  ') + C.dim('enter = now · any key = pause · esc = cancel'));
  const go = await holdOrContinue({ ms: 1500 });
  process.stdout.write('\n');
  if (!go) {
    stopProxy();
    console.log('Cancelled.');
    return 0;
  }

  // 4) Launch Claude Code pointed at the pool. Claude speaks the Anthropic API;
  //    the pool serves /v1/messages and routes across account OAuth tokens.
  const claude = which('claude');
  if (!claude) {
    stopProxy();
    throw new Error('The `claude` CLI was not found. Install Claude Code: https://claude.com/claude-code');
  }

  const env = { ...process.env };
  delete env.CLAUDE_CONFIG_DIR; // use the user's normal Claude Code workspace/config
  delete env.ANTHROPIC_API_KEY;
  delete env.CLAUDE_CODE_DISABLE_1M_CONTEXT;
  env.ANTHROPIC_BASE_URL = baseUrl;
  env.ANTHROPIC_AUTH_TOKEN = process.env.PROXY_API_KEY || 'claude-max-pool';
  env.NODE_NO_WARNINGS = '1';

  const claudeArgs = [...permissionArgs(permissionMode), ...extraArgs];

  console.log('Launching Claude Code through the account pool…\n');
  try {
    return await runInherit(claude, claudeArgs, env);
  } finally {
    stopProxy();
  }
}
