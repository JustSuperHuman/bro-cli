import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';
import { globalBinDirs, which, windowsCmdLine } from './proc.js';
import { table } from './table.js';

const MINUTE = 60 * 1000;
const DAY = 24 * 60 * MINUTE;
const CCUSAGE_VERSION = '20.0.19';
const CCUSAGE_PACKAGE = `ccusage@${CCUSAGE_VERSION}`;
const CCUSAGE_DAYS = 31;
const WINDOWS = [
  { key: 'hours24', label: 'Last 24h', days: 1 },
  { key: 'days7', label: 'Last 7d', days: 7 },
  { key: 'days30', label: 'Last 30d', days: 30 }
];
const moduleDir = path.dirname(fileURLToPath(import.meta.url));

const defaultClaudePool = () => process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
// Deliberately not CLAUDE_CONFIG_DIR / CODEX_HOME: bro sets those when it
// launches a pooled profile, so honoring them here would report that profile
// as "local" and drop the real local account from the report entirely.
const defaultClaudeHome = () => path.join(os.homedir(), '.claude');
const defaultCodexHome = () => path.join(os.homedir(), '.codex');
const defaultCodexProfiles = () => process.env.BRO_CODEX_PROFILES_DIR || path.join(BRO_DIR, 'codex-profiles');

const emptyUsage = () => ({
  inputTokens: 0,
  cachedTokens: 0,
  outputTokens: 0,
  totalTokens: 0
});

const finiteToken = (value) => {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? Math.round(number) : 0;
};

function normalizePath(value) {
  const resolved = path.resolve(value);
  return process.platform === 'win32' ? resolved.toLowerCase() : resolved;
}

function listDirectories(root) {
  try {
    return fs.readdirSync(root, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => ({ name: entry.name, dir: path.join(root, entry.name) }))
      .sort((a, b) => a.name.localeCompare(b.name));
  } catch {
    return [];
  }
}

export function listUsageProfiles({
  poolDir = defaultClaudePool(),
  claudeHome = defaultClaudeHome(),
  codexHome = defaultCodexHome(),
  codexProfilesDir = defaultCodexProfiles()
} = {}) {
  const profiles = [];
  const seen = new Set();
  const add = (provider, name, dir) => {
    if (!fs.existsSync(dir)) return;
    const key = `${provider}:${normalizePath(dir)}`;
    if (seen.has(key)) return;
    seen.add(key);
    profiles.push({ provider, name, dir });
  };

  add('Claude', 'local', claudeHome);
  for (const profile of listDirectories(path.join(poolDir, 'accounts'))) {
    add('Claude', profile.name, profile.dir);
  }
  add('Codex', 'local', codexHome);
  for (const profile of listDirectories(codexProfilesDir)) {
    add('Codex', profile.name, profile.dir);
  }
  return profiles;
}

// ccusage filters by calendar day. A fixed offset that makes the report minute
// exactly midnight turns those calendar buckets into precise rolling windows:
// yesterday is the last 24h, the previous seven dates are the last 7d, etc.
export function rollingAnchor(now = Date.now()) {
  const currentTime = now instanceof Date ? now.getTime() : Number(now);
  if (!Number.isFinite(currentTime)) throw new Error('Profiles report requires a valid current time.');
  const referenceMs = Math.floor(currentTime / MINUTE) * MINUTE;
  const utc = new Date(referenceMs);
  const utcMinute = utc.getUTCHours() * 60 + utc.getUTCMinutes();
  let offsetMinutes = -utcMinute;
  // Prefer the equivalent offset nearest UTC so it remains a conventional
  // fixed offset accepted by ccusage on every platform.
  if (offsetMinutes < -12 * 60) offsetMinutes += 24 * 60;
  const sign = offsetMinutes >= 0 ? '+' : '-';
  const absolute = Math.abs(offsetMinutes);
  const timezone = `${sign}${String(Math.floor(absolute / 60)).padStart(2, '0')}:${String(absolute % 60).padStart(2, '0')}`;
  const anchorDate = new Date(referenceMs + offsetMinutes * MINUTE).toISOString().slice(0, 10);
  return { referenceMs, timezone, anchorDate };
}

function dateBefore(anchorDate, days) {
  return new Date(Date.parse(`${anchorDate}T00:00:00Z`) - days * DAY).toISOString().slice(0, 10);
}

function addUsage(target, source) {
  target.inputTokens += source.inputTokens;
  target.cachedTokens += source.cachedTokens;
  target.outputTokens += source.outputTokens;
  target.totalTokens += source.totalTokens;
}

function normalizeDailyUsage(row) {
  const inputTokens = finiteToken(row?.inputTokens);
  const cachedTokens = finiteToken(row?.cacheCreationTokens) + finiteToken(row?.cacheReadTokens);
  const outputTokens = finiteToken(row?.outputTokens);
  return {
    inputTokens,
    cachedTokens,
    outputTokens,
    totalTokens: finiteToken(row?.totalTokens) || inputTokens + cachedTokens + outputTokens
  };
}

export function rollingUsageFromDaily(daily, anchorDate) {
  const byDate = new Map();
  for (const row of Array.isArray(daily) ? daily : []) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(row?.date || '')) continue;
    const usage = normalizeDailyUsage(row);
    const existing = byDate.get(row.date) || emptyUsage();
    addUsage(existing, usage);
    byDate.set(row.date, existing);
  }

  return Object.fromEntries(WINDOWS.map((window) => {
    const usage = emptyUsage();
    for (let day = 1; day <= window.days; day += 1) {
      const bucket = byDate.get(dateBefore(anchorDate, day));
      if (bucket) addUsage(usage, bucket);
    }
    return [window.key, usage];
  }));
}

export function resolveCcusageRunner({
  find = which,
  dirs = [path.resolve(moduleDir, '..', 'node_modules', '.bin'), ...globalBinDirs()]
} = {}) {
  const override = process.env.BRO_CCUSAGE_BIN;
  if (override) {
    const executable = fs.existsSync(override) ? override : find(override, dirs);
    if (!executable) throw new Error(`BRO_CCUSAGE_BIN does not exist: ${override}`);
    return { file: executable, prefix: [], label: 'ccusage' };
  }

  const direct = find('ccusage', dirs);
  if (direct) return { file: direct, prefix: [], label: `ccusage ${CCUSAGE_VERSION}` };
  // `bunx.exe` installed through WinGet loses its launcher name when spawned
  // directly on Windows and treats the package as a file. `bun x` is the same
  // package runner without relying on argv[0] symlink behavior.
  const bun = find('bun', dirs);
  if (bun) return { file: bun, prefix: ['x', CCUSAGE_PACKAGE], label: `ccusage ${CCUSAGE_VERSION}` };
  const npx = find('npx', dirs);
  if (npx) return { file: npx, prefix: ['--yes', CCUSAGE_PACKAGE], label: `ccusage ${CCUSAGE_VERSION}` };
  throw new Error('Profiles report requires ccusage. Install dependencies, or install Bun/npm so bro can run it.');
}

function runCaptured(file, args, env) {
  return new Promise((resolve, reject) => {
    const options = { env, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true };
    const extension = path.extname(file).toLowerCase();
    let child;
    if (process.platform === 'win32' && (extension === '.cmd' || extension === '.bat')) {
      child = spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', windowsCmdLine(file, args)], {
        ...options,
        windowsVerbatimArguments: true
      });
    } else {
      child = spawn(file, args, options);
    }

    let stdout = '';
    let stderr = '';
    let finished = false;
    child.stdout.setEncoding('utf8');
    child.stderr.setEncoding('utf8');
    child.stdout.on('data', (chunk) => { stdout += chunk; });
    child.stderr.on('data', (chunk) => { stderr += chunk; });
    child.on('error', (error) => {
      if (finished) return;
      finished = true;
      reject(error);
    });
    child.on('close', (code) => {
      if (finished) return;
      finished = true;
      if (code === 0) resolve(stdout);
      else reject(new Error(stderr.replace(/\x1b\[[0-9;]*m/g, '').trim() || `ccusage exited with code ${code}`));
    });
  });
}

function parseCcusageJson(output) {
  const text = String(output).trim();
  try {
    return JSON.parse(text);
  } catch {
    const start = text.indexOf('{');
    const end = text.lastIndexOf('}');
    if (start >= 0 && end > start) return JSON.parse(text.slice(start, end + 1));
    throw new Error('ccusage returned invalid JSON.');
  }
}

// Day rows for one profile. Omitting `last` asks for every day ccusage can
// still see, which is the most history a session-log analysis can offer.
export async function ccusageDaily({ provider, dir, last, timezone, runner = resolveCcusageRunner() }) {
  const args = [...runner.prefix, provider.toLowerCase(), 'daily', '--json', '--no-cost', '--offline'];
  if (last != null) args.push('--last', String(last));
  if (timezone) args.push(`--timezone=${timezone}`);
  const env = { ...process.env, NO_COLOR: '1' };
  if (provider === 'Claude') env.CLAUDE_CONFIG_DIR = dir;
  else env.CODEX_HOME = dir;
  const result = parseCcusageJson(await runCaptured(runner.file, args, env));
  if (!Array.isArray(result?.daily)) throw new Error('ccusage response did not contain daily usage.');
  return result.daily;
}

export async function analyzeUsageProfile(profile, { anchor, runner = resolveCcusageRunner() } = {}) {
  if (!anchor) throw new Error('ccusage profile analysis requires a rolling anchor.');
  return ccusageDaily({
    provider: profile.provider,
    dir: profile.dir,
    last: CCUSAGE_DAYS,
    timezone: anchor.timezone,
    runner
  });
}

export async function buildProfilesReport({
  poolDir = defaultClaudePool(),
  claudeHome = defaultClaudeHome(),
  codexHome = defaultCodexHome(),
  codexProfilesDir = defaultCodexProfiles(),
  now = Date.now(),
  analyzer,
  runner
} = {}) {
  const anchor = rollingAnchor(now);
  const profiles = listUsageProfiles({ poolDir, claudeHome, codexHome, codexProfilesDir });
  const resolvedRunner = analyzer ? null : (runner || resolveCcusageRunner());
  const analyze = analyzer || ((profile) => analyzeUsageProfile(profile, { anchor, runner: resolvedRunner }));
  const rows = [];

  // Keep this sequential: a fresh bunx/npx cache may need to install the pinned
  // analyzer once, and parallel installers can contend for that cache.
  for (const profile of profiles) {
    try {
      const result = await analyze(profile, { anchor });
      const daily = Array.isArray(result) ? result : result?.daily;
      if (!Array.isArray(daily)) throw new Error('usage analyzer returned no daily rows');
      rows.push({
        provider: profile.provider,
        profile: profile.name,
        available: true,
        usage: rollingUsageFromDaily(daily, anchor.anchorDate)
      });
    } catch (error) {
      rows.push({
        provider: profile.provider,
        profile: profile.name,
        available: false,
        error: error instanceof Error ? error.message : String(error),
        usage: Object.fromEntries(WINDOWS.map((window) => [window.key, emptyUsage()]))
      });
    }
  }

  const totals = Object.fromEntries(WINDOWS.map((window) => [window.key, emptyUsage()]));
  for (const row of rows.filter((candidate) => candidate.available)) {
    for (const window of WINDOWS) addUsage(totals[window.key], row.usage[window.key]);
  }
  return {
    generatedAt: new Date(anchor.referenceMs).toISOString(),
    windows: WINDOWS.map(({ key, label, days }) => ({ key, label, days })),
    rows,
    totals,
    complete: rows.every((row) => row.available),
    source: resolvedRunner?.label || `ccusage ${CCUSAGE_VERSION}`,
    analyzerTimezone: anchor.timezone
  };
}

const formatNumber = (value) => new Intl.NumberFormat('en-US').format(value);

function formatReference(value) {
  return new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: 'short', day: 'numeric',
    hour: 'numeric', minute: '2-digit', timeZoneName: 'short'
  }).format(new Date(value));
}

export function formatProfilesReport(report) {
  if (!report.rows.length) return 'Profiles\n\nNo Claude or Codex profiles found.';
  let previousProvider = null;
  const rows = report.rows.map((row) => {
    const provider = row.provider === previousProvider ? '' : row.provider;
    previousProvider = row.provider;
    return [
      provider,
      row.profile,
      row.available ? formatNumber(row.usage.hours24.totalTokens) : '—',
      row.available ? formatNumber(row.usage.days7.totalTokens) : '—',
      row.available ? formatNumber(row.usage.days30.totalTokens) : '—'
    ];
  });
  rows.push([
    'All',
    'TOTAL',
    formatNumber(report.totals.hours24.totalTokens),
    formatNumber(report.totals.days7.totalTokens),
    formatNumber(report.totals.days30.totalTokens)
  ]);
  return [
    'Profiles · total token usage',
    '',
    table({
      headers: ['Provider', 'Profile', 'Last 24h', 'Last 7d', 'Last 30d'],
      rows,
      numeric: [2, 3, 4],
      rules: [rows.length - 1]
    }),
    '',
    `Rolling windows end ${formatReference(report.generatedAt)} (minute precision).`,
    `Totals include input, cached, and output tokens. Source: ${report.source} local analysis.`
  ].join('\n');
}

export async function runProfilesReport(options) {
  console.log('Analyzing Claude and Codex profiles with ccusage…');
  const report = await buildProfilesReport(options);
  console.log(`\n${formatProfilesReport(report)}`);
  for (const row of report.rows.filter((candidate) => !candidate.available)) {
    console.log(`\nWarning: ${row.provider} ${row.profile}: ${row.error}`);
  }
  return 0;
}
