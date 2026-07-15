import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { buildTokenReport, claudeCliStats, parseClaudeStatsTotal } from './token-report.js';

function fixture() { return fs.mkdtempSync(path.join(os.tmpdir(), 'bro-token-report-')); }

test('Claude lifetime total comes from its CLI stats cache', () => {
  const root = fixture();
  fs.writeFileSync(path.join(root, 'stats-cache.json'), JSON.stringify({
    lastComputedDate: '2026-07-12', totalSessions: 4,
    dailyModelTokens: [{ date: '2026-07-12', tokensByModel: { opus: 12, fable: 8 } }]
  }));
  expect(claudeCliStats(root)).toEqual({ available: true, totalTokens: 20, sessions: 4, through: '2026-07-12' });
});

test('missing CLI stats are unavailable instead of estimated from history', () => {
  expect(claudeCliStats(fixture()).available).toBe(false);
});

test('Claude stats screen totals parse its compact lifetime units', () => {
  expect(parseClaudeStatsTotal('Favorite model: Fable 5 Total tokens: 278.9k')).toBe(278900);
  expect(parseClaudeStatsTotal('Total tokens: 12.4m')).toBe(12400000);
});

test('report loads every Claude switcher profile and detects Codex without estimating it', async () => {
  const root = fixture();
  const poolDir = path.join(root, 'pool');
  const codexHome = path.join(root, 'codex');
  for (const [name, count] of [['work', 7], ['personal', 9]]) {
    const dir = path.join(poolDir, 'accounts', name);
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'stats-cache.json'), JSON.stringify({ dailyModelTokens: [{ tokensByModel: { model: count } }] }));
  }
  fs.mkdirSync(codexHome, { recursive: true });
  const report = await buildTokenReport({ poolDir, claudeHome: path.join(root, 'missing-default'), codexHome, refresh: false });
  expect(report.profiles.map((p) => p.name)).toEqual(['personal', 'work']);
  expect(report.total).toBe(16);
  expect(report.codex.available).toBe(false);
});
