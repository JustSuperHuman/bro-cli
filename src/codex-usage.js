import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { codexAuthStatus, freshCodexAuth } from './codex-auth.js';
import { globalBinDirs, which, windowsCmdLine } from './proc.js';

const finite = (value) => Number.isFinite(Number(value)) ? Number(value) : null;

function resetTimestamp(value, resetAfterSeconds) {
  const numeric = finite(value);
  if (numeric != null) return numeric;
  if (typeof value === 'string') {
    const parsed = Date.parse(value);
    if (Number.isFinite(parsed)) return Math.floor(parsed / 1000);
  }
  const after = finite(resetAfterSeconds);
  return after == null ? null : Math.floor(Date.now() / 1000 + after);
}

function normalizeWindow(value) {
  if (!value || typeof value !== 'object') return null;
  const seconds = finite(value.limit_window_seconds ?? value.window_duration_seconds);
  return {
    usedPercent: finite(value.used_percent ?? value.usedPercent),
    windowDurationMins: seconds == null
      ? finite(value.window_minutes ?? value.windowDurationMins)
      : Math.round(seconds / 60),
    resetsAt: resetTimestamp(value.reset_at ?? value.resets_at ?? value.resetsAt, value.reset_after_seconds)
  };
}

// Normalize Codex's account meter into the same compact shape the DSH client
// consumes. Both snake_case backend responses and the app-server camelCase
// projection are accepted so the parser remains useful across Codex versions.
export function codexUsageSummary(payload) {
  const rate = payload?.rateLimitsByLimitId?.codex
    ?? payload?.rate_limit
    ?? payload?.rateLimits
    ?? payload
    ?? {};
  const primary = normalizeWindow(rate.primary_window ?? rate.primary);
  const secondary = normalizeWindow(rate.secondary_window ?? rate.secondary);
  const credits = payload?.credits ?? rate.credits ?? null;
  return {
    primary,
    secondary,
    planType: payload?.plan_type ?? rate.planType ?? null,
    credits: credits && typeof credits === 'object'
      ? { balance: finite(credits.balance), unlimited: credits.unlimited === true }
      : null
  };
}

function spawnAppServer(executable, env, spawnProcess) {
  const args = ['app-server', '--stdio'];
  if (process.platform === 'win32' && ['.cmd', '.bat'].includes(path.extname(executable).toLowerCase())) {
    return spawnProcess(process.env.ComSpec || 'cmd.exe', [
      '/d', '/s', '/c', windowsCmdLine(executable, args)
    ], {
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      windowsVerbatimArguments: true
    });
  }
  return spawnProcess(executable, args, {
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true
  });
}

async function appServerHome(home) {
  if (home) return { home: path.resolve(home), cleanup: null };
  let status = codexAuthStatus();
  if (!status.loggedIn || !status.source) throw new Error('Codex is not logged in');
  // The Codex CLI's own auth.json already belongs to a valid CODEX_HOME.
  if (path.basename(status.source).toLowerCase() === 'auth.json') {
    return { home: path.dirname(status.source), cleanup: null };
  }

  // bro's self-contained local login uses codex-auth.json. Refresh it first,
  // then stage a launch-only Codex home so the official app-server can read
  // the same credential without moving or renaming the user's real store.
  await freshCodexAuth();
  status = codexAuthStatus();
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-codex-usage-'));
  try {
    const target = path.join(root, 'auth.json');
    fs.copyFileSync(status.source, target);
    try { fs.chmodSync(target, 0o600); } catch {}
    return {
      home: root,
      cleanup: () => fs.rmSync(root, { recursive: true, force: true })
    };
  } catch (error) {
    fs.rmSync(root, { recursive: true, force: true });
    throw error;
  }
}

// Codex exposes account quotas through its supported app-server protocol.
// Using account/rateLimits/read keeps profile isolation, response evolution,
// and authentication behavior owned by the installed Codex version instead
// of duplicating a private backend request in bro.
export async function fetchCodexUsage({
  home = '',
  timeoutMs = 6000,
  executable = which('codex', globalBinDirs()),
  spawnProcess = spawn
} = {}) {
  if (!executable) throw new Error('Codex CLI is not installed');
  const prepared = await appServerHome(home);
  const child = spawnAppServer(executable, {
    ...process.env,
    CODEX_HOME: prepared.home
  }, spawnProcess);
  const pending = new Map();
  const deadline = Date.now() + timeoutMs;
  let stdout = '';
  let stderr = '';
  let nextId = 0;
  const exited = new Promise((resolve) => child.once('exit', resolve));

  const failPending = (error) => {
    for (const entry of pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(error);
    }
    pending.clear();
  };
  child.stderr?.on('data', (chunk) => {
    stderr = `${stderr}${chunk}`.slice(-4096);
  });
  child.stdout?.on('data', (chunk) => {
    stdout += chunk;
    let index;
    while ((index = stdout.indexOf('\n')) >= 0) {
      const line = stdout.slice(0, index).trim();
      stdout = stdout.slice(index + 1);
      if (!line) continue;
      let message;
      try { message = JSON.parse(line); } catch { continue; }
      const entry = pending.get(message.id);
      if (!entry) continue;
      pending.delete(message.id);
      clearTimeout(entry.timer);
      if (message.error) entry.reject(new Error(message.error.message || 'Codex app-server request failed'));
      else entry.resolve(message.result);
    }
  });
  child.on('error', (error) => failPending(error));
  child.on('exit', (code) => {
    if (pending.size) {
      failPending(new Error(`Codex app-server exited (${code ?? 'unknown'})${stderr.trim() ? `: ${stderr.trim()}` : ''}`));
    }
  });

  const send = (method, params) => new Promise((resolve, reject) => {
    const remaining = deadline - Date.now();
    if (remaining <= 0) {
      reject(new Error(`Codex app-server timed out during ${method}`));
      return;
    }
    const id = nextId++;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Codex app-server timed out during ${method}`));
    }, remaining);
    pending.set(id, { resolve, reject, timer });
    try {
      child.stdin.write(`${JSON.stringify({ method, id, ...(params === undefined ? {} : { params }) })}\n`);
    } catch (error) {
      clearTimeout(timer);
      pending.delete(id);
      reject(error);
    }
  });

  try {
    await send('initialize', {
      clientInfo: { name: 'bro_cli', title: 'bro CLI', version: '0.4.3' }
    });
    child.stdin.write(`${JSON.stringify({ method: 'initialized' })}\n`);
    return codexUsageSummary(await send('account/rateLimits/read'));
  } finally {
    for (const entry of pending.values()) clearTimeout(entry.timer);
    pending.clear();
    try { child.stdin.end(); } catch {}
    if (child.exitCode === null) {
      try { child.kill(); } catch {}
    }
    await Promise.race([
      exited,
      new Promise((resolve) => {
        const timer = setTimeout(resolve, 1000);
        timer.unref?.();
      })
    ]);
    prepared.cleanup?.();
  }
}
