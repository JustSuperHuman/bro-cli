import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';

const CLAUDE_POOL_DIR = () => process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
const CODEX_HOME = () => process.env.CODEX_HOME || path.join(os.homedir(), '.codex');

export function claudeCliStats(profileDir) {
  try {
    const stats = JSON.parse(fs.readFileSync(path.join(profileDir, 'stats-cache.json'), 'utf8'));
    const totalTokens = (stats.dailyModelTokens || []).reduce((total, day) =>
      total + Object.values(day?.tokensByModel || {}).reduce((sum, value) => sum + (Number.isFinite(value) ? value : 0), 0), 0);
    return {
      available: true,
      totalTokens,
      sessions: Number.isFinite(stats.totalSessions) ? stats.totalSessions : null,
      through: stats.lastComputedDate || null
    };
  } catch {
    return { available: false, outputTokens: null, sessions: null, through: null };
  }
}

export function parseClaudeStatsTotal(text) {
  const matches = [...String(text).matchAll(/Total tokens:\s*([\d,.]+)\s*([kmb])?/gi)];
  const match = matches.at(-1);
  if (!match) return null;
  const base = Number(match[1].replace(/,/g, ''));
  const multiplier = { k: 1e3, m: 1e6, b: 1e9 }[match[2]?.toLowerCase()] || 1;
  return Number.isFinite(base) ? Math.round(base * multiplier) : null;
}

function findWinpty() {
  const candidates = [
    path.join(process.env.ProgramFiles || 'C:\\Program Files', 'Git', 'usr', 'bin', 'winpty.exe'),
    path.join(process.env.LOCALAPPDATA || '', 'Programs', 'Git', 'usr', 'bin', 'winpty.exe')
  ];
  return candidates.find((candidate) => fs.existsSync(candidate)) || null;
}

export function refreshClaudeCliStats(profile, { timeoutMs = 90000 } = {}) {
  const winpty = process.platform === 'win32' ? findWinpty() : null;
  if (!winpty) return Promise.resolve({ available: false, reason: 'automatic /stats refresh requires Git for Windows (winpty)' });

  return new Promise((resolve) => {
    let output = '';
    let sentTrust = false;
    let sentStats = false;
    let cursorQueriesAnswered = 0;
    const rangeTotals = [];
    let cyclingRanges = false;
    let settled = false;
    const env = { ...process.env };
    if (profile.name === 'default') delete env.CLAUDE_CONFIG_DIR;
    else env.CLAUDE_CONFIG_DIR = profile.dir;
    const child = spawn(winpty, ['-Xallow-non-tty', 'claude', '--dangerously-skip-permissions', '--ax-screen-reader'], {
      cwd: profile.dir,
      env,
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true
    });
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill();
      resolve(result);
    };
    const inspect = (chunk) => {
      output = (output + chunk.toString()).slice(-100000);
      const cursorQueries = (output.match(/\x1b\[6n/g) || []).length;
      while (cursorQueriesAnswered < cursorQueries) {
        cursorQueriesAnswered++;
        child.stdin.write('\x1b[1;1R');
      }
      if (!sentTrust && /Enter y\/n:/.test(output)) {
        sentTrust = true;
        child.stdin.write('y\r');
      }
      if (!sentStats && (/effort:|Tips for getting started|What's new/.test(output))) {
        sentStats = true;
        setTimeout(() => { if (!settled) child.stdin.write('/stats\r'); }, 300);
      }
      const totalTokens = parseClaudeStatsTotal(output);
      if (totalTokens !== null && rangeTotals.at(-1) !== totalTokens) rangeTotals.push(totalTokens);
      if (totalTokens !== null && !cyclingRanges) {
        cyclingRanges = true;
        // Ink only emits changed lines. A date range with the same total may
        // therefore produce no second "Total tokens" line, so cycle on fixed
        // beats and collect every total that is actually redrawn.
        setTimeout(() => { if (!settled) child.stdin.write('r'); }, 400);
        setTimeout(() => { if (!settled) child.stdin.write('r'); }, 1000);
        setTimeout(() => {
          if (!settled) finish({ available: true, totalTokens: Math.max(...rangeTotals) });
        }, 2200);
      }
    };
    child.stdout.on('data', inspect);
    child.stderr.on('data', inspect);
    child.on('error', (error) => finish({ available: false, reason: error.message }));
    child.on('exit', () => finish({ available: false, reason: 'Claude /stats exited before returning a total' }));
    const timer = setTimeout(() => finish({ available: false, reason: 'Claude /stats timed out' }), timeoutMs);
  });
}

function claudeProfiles(poolDir = CLAUDE_POOL_DIR()) {
  const accountsDir = path.join(poolDir, 'accounts');
  try {
    return fs.readdirSync(accountsDir, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => ({ name: entry.name, dir: path.join(accountsDir, entry.name) }))
      .sort((a, b) => a.name.localeCompare(b.name));
  } catch { return []; }
}

const number = (value) => new Intl.NumberFormat('en-US').format(value);

export async function buildTokenReport({ poolDir = CLAUDE_POOL_DIR(), claudeHome = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude'), codexHome = CODEX_HOME(), refresh = true } = {}) {
  const found = claudeProfiles(poolDir);
  if (fs.existsSync(claudeHome)) found.unshift({ name: 'default', dir: claudeHome });
  const profiles = [];
  // Keep concurrent CLI scans bounded: each /stats invocation reads that
  // profile's history and six simultaneous scans can thrash slower disks.
  for (let i = 0; i < found.length; i += 2) {
    const batch = found.slice(i, i + 2);
    const results = await Promise.all(batch.map(async (profile) => {
      if (refresh) {
        const live = await refreshClaudeCliStats(profile, { timeoutMs: profile.name === 'default' ? 5 * 60 * 1000 : 90000 });
        if (live.available) return { provider: 'Claude', name: profile.name, available: true, totalTokens: live.totalTokens, source: 'live /stats' };
      }
      const cached = claudeCliStats(profile.dir);
      return { provider: 'Claude', name: profile.name, ...cached, source: cached.available ? 'CLI stats cache' : null };
    }));
    profiles.push(...results);
  }
  const codexInstalled = fs.existsSync(codexHome);
  // Codex 0.144.1 exposes neither a status subcommand nor a persisted lifetime
  // counter. Do not present a sum of retained session logs as actual lifetime.
  const codex = { installed: codexInstalled, available: false, outputTokens: null };
  const available = profiles.filter((profile) => profile.available);
  return { profiles, codex, total: available.reduce((sum, p) => sum + p.totalTokens, 0), complete: available.length === profiles.length && codex.available };
}

export async function runTokenReport(options) {
  console.log('Loading CLI lifetime token stats…');
  const report = await buildTokenReport(options);
  console.log('\nLifetime tokens');
  if (!report.profiles.length) console.log('  Claude profiles  none found');
  for (const profile of report.profiles) {
    const detail = profile.available
      ? `${number(profile.totalTokens)}  (${profile.source}${profile.through ? ` through ${profile.through}` : ''})`
      : 'unavailable — Claude /stats did not return a total';
    console.log(`  Claude / ${profile.name}  ${detail}`);
  }
  console.log(report.codex.installed
    ? '  Codex             unavailable — installed CLI exposes no lifetime total'
    : '  Codex             not installed');
  if (report.complete) console.log(`  Total             ${number(report.total)}`);
  else if (report.total > 0) console.log(`  Known subtotal    ${number(report.total)}`);
  console.log('\nOnly CLI-owned lifetime counters are reported; retained session logs are not used as a substitute.');
  return 0;
}
