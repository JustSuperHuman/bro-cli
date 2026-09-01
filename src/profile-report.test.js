import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  buildProfilesReport,
  formatProfilesReport,
  listUsageProfiles,
  resolveCcusageRunner,
  rollingAnchor,
  rollingUsageFromDaily
} from './profile-report.js';

function fixture() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'bro-profile-report-'));
}

test('profile discovery includes local and managed Claude and Codex homes', () => {
  const root = fixture();
  const claudeHome = path.join(root, 'claude');
  const poolDir = path.join(root, 'pool');
  const codexHome = path.join(root, 'codex');
  const codexProfilesDir = path.join(root, 'codex-profiles');
  for (const dir of [claudeHome, path.join(poolDir, 'accounts', 'work'), codexHome, path.join(codexProfilesDir, 'team')]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  expect(listUsageProfiles({ poolDir, claudeHome, codexHome, codexProfilesDir }).map((profile) => `${profile.provider}/${profile.name}`))
    .toEqual(['Claude/local', 'Claude/work', 'Codex/local', 'Codex/team']);
});

test('rolling anchor aligns the report minute to a ccusage day boundary', () => {
  expect(rollingAnchor(Date.parse('2026-08-14T12:34:56Z'))).toEqual({
    referenceMs: Date.parse('2026-08-14T12:34:00Z'),
    timezone: '+11:26',
    anchorDate: '2026-08-15'
  });
  expect(rollingAnchor(Date.parse('2026-08-14T01:05:09Z'))).toEqual({
    referenceMs: Date.parse('2026-08-14T01:05:00Z'),
    timezone: '-01:05',
    anchorDate: '2026-08-14'
  });
});

test('Bun fallback uses bun x so Windows launcher links work when spawned', () => {
  const runner = resolveCcusageRunner({
    dirs: [],
    find: (name) => name === 'bun' ? 'C:\\tools\\bun.exe' : null
  });
  expect(runner).toEqual({
    file: 'C:\\tools\\bun.exe',
    prefix: ['x', 'ccusage@20.0.19'],
    label: 'ccusage 20.0.19'
  });
});

test('ccusage day rows become exact rolling 24h, 7d and 30d totals', () => {
  const daily = [
    { date: '2026-08-15', totalTokens: 999 }, // current partial minute is intentionally excluded
    { date: '2026-08-14', inputTokens: 2, cacheReadTokens: 3, outputTokens: 5, totalTokens: 10 },
    { date: '2026-08-13', totalTokens: 20 },
    { date: '2026-08-08', totalTokens: 70 },
    { date: '2026-08-07', totalTokens: 80 },
    { date: '2026-07-16', totalTokens: 300 }
  ];
  const usage = rollingUsageFromDaily(daily, '2026-08-15');
  expect(usage.hours24).toEqual({ inputTokens: 2, cachedTokens: 3, outputTokens: 5, totalTokens: 10 });
  expect(usage.days7.totalTokens).toBe(100);
  expect(usage.days30.totalTokens).toBe(480);
});

test('profiles report uses analyzer totals for every profile and isolates failures', async () => {
  const root = fixture();
  const claudeHome = path.join(root, 'claude');
  const poolDir = path.join(root, 'pool');
  const codexHome = path.join(root, 'codex');
  const codexProfilesDir = path.join(root, 'codex-profiles');
  for (const dir of [claudeHome, path.join(poolDir, 'accounts', 'work'), codexHome, path.join(codexProfilesDir, 'team')]) {
    fs.mkdirSync(dir, { recursive: true });
  }

  const totals = new Map([
    ['Claude/local', 10],
    ['Claude/work', 20],
    ['Codex/local', 30]
  ]);
  const report = await buildProfilesReport({
    poolDir,
    claudeHome,
    codexHome,
    codexProfilesDir,
    now: Date.parse('2026-08-14T12:34:56Z'),
    analyzer: async (profile, { anchor }) => {
      const key = `${profile.provider}/${profile.name}`;
      if (key === 'Codex/team') throw new Error('broken profile');
      return [{ date: new Date(Date.parse(`${anchor.anchorDate}T00:00:00Z`) - 24 * 60 * 60 * 1000).toISOString().slice(0, 10), totalTokens: totals.get(key) }];
    }
  });

  expect(report.rows.map((row) => `${row.provider}/${row.profile}`))
    .toEqual(['Claude/local', 'Claude/work', 'Codex/local', 'Codex/team']);
  expect(report.rows.map((row) => row.usage.hours24.totalTokens)).toEqual([10, 20, 30, 0]);
  expect(report.rows.at(-1)).toMatchObject({ available: false, error: 'broken profile' });
  expect(report.totals.hours24.totalTokens).toBe(60);
  expect(report.totals.days7.totalTokens).toBe(60);
  expect(report.totals.days30.totalTokens).toBe(60);
  expect(report.complete).toBe(false);

  const output = formatProfilesReport(report);
  expect(output).toContain('Profiles · total token usage');
  expect(output).toContain('Last 24h');
  expect(output).toContain('Claude');
  expect(output).toContain('Codex');
  expect(output).toContain('ccusage 20.0.19');
});
