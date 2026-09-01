import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { buildTokenReport, claudeCliStats, codexLogStats, formatTokenReport } from './token-report.js';

function fixture() { return fs.mkdtempSync(path.join(os.tmpdir(), 'bro-token-report-')); }

const NOW = new Date(2026, 6, 12, 12, 0, 0).getTime(); // local noon on 2026-07-12
const WINDOW = { from: '2026-06-13', to: '2026-07-12' };

function writeStats(dir, stats) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'stats-cache.json'), JSON.stringify(stats));
  return dir;
}

function layout(root, { claude = {}, codex = [] } = {}) {
  const dirs = {
    poolDir: path.join(root, 'pool'),
    claudeHome: path.join(root, 'home', '.claude'),
    codexHome: path.join(root, 'home', '.codex'),
    codexProfilesDir: path.join(root, 'codex-profiles')
  };
  for (const [name, stats] of Object.entries(claude)) {
    const dir = name === 'local' ? dirs.claudeHome : path.join(dirs.poolDir, 'accounts', name);
    if (stats) writeStats(dir, stats);
    else fs.mkdirSync(dir, { recursive: true });
  }
  for (const name of codex) {
    fs.mkdirSync(name === 'local' ? dirs.codexHome : path.join(dirs.codexProfilesDir, name), { recursive: true });
  }
  return dirs;
}

test('Claude lifetime comes from the all-time per-model counter, not the retained daily rows', () => {
  const root = writeStats(fixture(), {
    lastComputedDate: '2026-07-12',
    totalSessions: 4,
    modelUsage: {
      opus: { inputTokens: 1, outputTokens: 2, cacheReadInputTokens: 300, cacheCreationInputTokens: 40, costUSD: 9 },
      fable: { inputTokens: 5, outputTokens: 6, cacheReadInputTokens: 700, cacheCreationInputTokens: 80 }
    },
    dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { opus: 12, fable: 8 } }]
  });
  expect(claudeCliStats(root, { now: NOW })).toEqual({
    available: true,
    source: 'counter',
    lifetimeTokens: 1134,
    windowTokens: 20,
    sessions: 4,
    through: '2026-07-12'
  });
});

test('the reported window only counts days inside it', () => {
  const root = writeStats(fixture(), {
    modelUsage: { opus: { inputTokens: 1000 } },
    dailyModelTokens: [
      { date: '2026-06-12', tokensByModel: { opus: 400 } }, // one day too old
      { date: '2026-06-13', tokensByModel: { opus: 30 } },  // first day in range
      { date: '2026-07-12', tokensByModel: { opus: 7 } },   // today
      { date: '2026-07-13', tokensByModel: { opus: 500 } }  // future-dated
    ]
  });
  const stats = claudeCliStats(root, { now: NOW });
  expect(stats.windowTokens).toBe(37);
  expect(stats.lifetimeTokens).toBe(1000);
});

test('missing CLI stats are unavailable instead of estimated from history', () => {
  expect(claudeCliStats(fixture())).toMatchObject({ available: false, reason: 'no CLI stats cache yet' });
});

test('malformed token counters are skipped rather than poisoning the total', () => {
  const root = writeStats(fixture(), {
    modelUsage: { opus: { inputTokens: 'lots', outputTokens: 5, cacheReadInputTokens: null } },
    dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { opus: undefined, fable: 3 } }]
  });
  const stats = claudeCliStats(root, { now: NOW });
  expect(stats.lifetimeTokens).toBe(5);
  expect(stats.windowTokens).toBe(3);
});

test('Codex totals come from every retained session-log day, windowed the same way', () => {
  const daily = [
    { date: '2026-05-01', totalTokens: 900 },                                   // before the window
    { date: '2026-06-13', totalTokens: 60 },
    { date: '2026-07-12', inputTokens: 1, outputTokens: 2, cacheReadTokens: 3, cacheCreationTokens: 4 }
  ];
  expect(codexLogStats(daily, { now: NOW })).toEqual({
    available: true,
    source: 'logs',
    lifetimeTokens: 970,
    windowTokens: 70,
    sessions: null,
    through: '2026-07-12'
  });
});

test('report reads Claude from its counters and Codex from session logs, then totals both', async () => {
  const root = fixture();
  const dirs = layout(root, {
    claude: {
      local: { modelUsage: { m: { inputTokens: 500 } }, dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { m: 100 } }] },
      work: { modelUsage: { m: { inputTokens: 300 } }, dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { m: 60 } }] },
      'never-launched': null
    },
    codex: ['local']
  });

  const report = await buildTokenReport({
    ...dirs,
    now: NOW,
    analyzeCodex: async () => [{ date: '2026-07-12', totalTokens: 40 }, { date: '2026-01-01', totalTokens: 200 }]
  });

  expect(report.profiles.map((profile) => `${profile.provider}/${profile.name}`))
    .toEqual(['Claude/local', 'Claude/never-launched', 'Claude/work', 'Codex/local']);
  expect(report.window).toEqual(WINDOW);
  expect(report.providers).toEqual([
    { provider: 'Claude', source: 'counter', count: 3, lifetimeTokens: 800, windowTokens: 160 },
    { provider: 'Codex', source: 'logs', count: 1, lifetimeTokens: 240, windowTokens: 40 }
  ]);
  expect(report.lifetimeTotal).toBe(1040);
  expect(report.windowTotal).toBe(200);
  expect(report.complete).toBe(false); // never-launched has no cache
});

test('a Codex analyzer failure is isolated to its own row', async () => {
  const root = fixture();
  const dirs = layout(root, { claude: { local: { modelUsage: { m: { inputTokens: 5 } } } }, codex: ['local', 'team'] });
  const report = await buildTokenReport({
    ...dirs,
    now: NOW,
    analyzeCodex: async (profile) => {
      if (profile.name === 'team') throw new Error('ccusage is not installed');
      return [{ date: '2026-07-12', totalTokens: 40 }];
    }
  });

  expect(report.profiles.at(-1)).toMatchObject({
    provider: 'Codex',
    name: 'team',
    available: false,
    reason: 'ccusage is not installed'
  });
  expect(report.lifetimeTotal).toBe(45);
  expect(formatTokenReport(report)).toContain('ccusage is not installed');
});

test('the local account row ignores an inherited pool CLAUDE_CONFIG_DIR', async () => {
  const root = fixture();
  const dirs = layout(root, {
    claude: { local: { modelUsage: { m: { inputTokens: 11 } } }, pooled: { modelUsage: { m: { inputTokens: 22 } } } }
  });

  const previous = process.env.CLAUDE_CONFIG_DIR;
  process.env.CLAUDE_CONFIG_DIR = path.join(dirs.poolDir, 'accounts', 'pooled');
  try {
    const report = await buildTokenReport({ ...dirs, now: NOW });
    expect(report.profiles.map((profile) => profile.name)).toEqual(['local', 'pooled']);
    expect(report.profiles[0].lifetimeTokens).toBe(11);
    expect(report.lifetimeTotal).toBe(33);
  } finally {
    if (previous === undefined) delete process.env.CLAUDE_CONFIG_DIR;
    else process.env.CLAUDE_CONFIG_DIR = previous;
  }
});

test('the formatted table bands each provider, subtotals only multi-profile ones, and flags stale caches', async () => {
  const root = fixture();
  const dirs = layout(root, {
    claude: {
      local: { lastComputedDate: '2026-07-12', modelUsage: { m: { inputTokens: 750 } }, dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { m: 75 } }] },
      stale: { lastComputedDate: '2026-05-01', modelUsage: { m: { inputTokens: 250 } }, dailyModelTokens: [{ date: '2026-05-01', tokensByModel: { m: 250 } }] }
    },
    codex: ['local']
  });
  const report = await buildTokenReport({ ...dirs, now: NOW, analyzeCodex: async () => [{ date: '2026-07-12', totalTokens: 25 }] });
  const output = formatTokenReport(report);

  expect(output).toContain('Token totals · lifetime and the last 30 days');
  expect(output).toContain('30d share');
  expect(output).toContain('Claude   │ subtotal');       // two Claude profiles earn a subtotal
  expect(output).not.toContain('Codex    │ subtotal');   // the lone Codex profile does not
  expect(output).toContain('All      │ TOTAL');
  expect(output).toContain('100%');
  expect(output).toContain('no activity recorded since 2026-05-01');
  expect(output).toContain(`Window ${WINDOW.from} → ${WINDOW.to}`);
  expect(output).toContain('ccusage');
  // 75 of a 100-token window is three quarters of the bar.
  expect(output).toContain('███████▌░░  75%');
});

test('a machine with no profiles at all says so instead of rendering an empty table', async () => {
  const root = fixture();
  const report = await buildTokenReport({
    poolDir: path.join(root, 'none'),
    claudeHome: path.join(root, 'none'),
    codexHome: path.join(root, 'none'),
    codexProfilesDir: path.join(root, 'none'),
    now: NOW
  });
  expect(report.profiles).toEqual([]);
  expect(formatTokenReport(report)).toContain('No Claude or Codex profiles found.');
});
