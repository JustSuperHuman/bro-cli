import fs from 'node:fs';
import path from 'node:path';
import { ccusageDaily, listUsageProfiles, resolveCcusageRunner } from './profile-report.js';
import { bar, table } from './table.js';

// Claude's stats cache carries two independent counters. `modelUsage` is the
// all-time per-model tally and never rolls off; `dailyModelTokens` is a short
// retention window that the /stats screen slices to fill its date ranges. For
// every model whose whole history still fits inside the retained days the two
// agree exactly, which is what makes these four fields the right lifetime sum.
// Adding up the daily rows is neither figure — not lifetime, and not the range
// /stats shows — so read each counter for what it actually is.
const LIFETIME_FIELDS = ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens'];
const WINDOW_DAYS = 30;
const DAY = 24 * 60 * 60 * 1000;

// Codex keeps no lifetime counter of its own, so its numbers are reconstructed
// from retained session logs. That is a floor, not a ledger, and the report
// labels it as such rather than quietly mixing it in with Claude's counters.
const SOURCES = {
  counter: { key: 'counter', label: 'CLI counter' },
  logs: { key: 'logs', label: 'session logs' }
};

const finite = (value) => (Number.isFinite(Number(value)) ? Number(value) : 0);

// Claude and ccusage both bucket their day rows by local date, so the window
// bounds have to be local dates too — a UTC cutoff moves the boundary by a day.
function localDate(ms) {
  const at = new Date(ms);
  return new Date(at.getTime() - at.getTimezoneOffset() * 60000).toISOString().slice(0, 10);
}

function windowBounds(now, days) {
  return { from: localDate(now - (days - 1) * DAY), to: localDate(now) };
}

const inWindow = (date, { from, to }) => typeof date === 'string' && date >= from && date <= to;

function lifetimeTokens(modelUsage) {
  return Object.values(modelUsage || {}).reduce((total, usage) =>
    total + LIFETIME_FIELDS.reduce((sum, field) => sum + finite(usage?.[field]), 0), 0);
}

function claudeDailyTokens(dailyModelTokens, bounds) {
  return (Array.isArray(dailyModelTokens) ? dailyModelTokens : [])
    .filter((day) => inWindow(day?.date, bounds))
    .reduce((total, day) =>
      total + Object.values(day.tokensByModel || {}).reduce((sum, value) => sum + finite(value), 0), 0);
}

function ccusageTokens(daily, bounds) {
  return (Array.isArray(daily) ? daily : [])
    .filter((day) => (bounds ? inWindow(day?.date, bounds) : typeof day?.date === 'string'))
    .reduce((total, day) => total + (finite(day.totalTokens)
      || finite(day.inputTokens) + finite(day.outputTokens)
        + finite(day.cacheReadTokens) + finite(day.cacheCreationTokens)), 0);
}

const unavailable = (extra) => ({
  available: false,
  lifetimeTokens: null,
  windowTokens: null,
  sessions: null,
  through: null,
  ...extra
});

export function claudeCliStats(profileDir, { now = Date.now(), days = WINDOW_DAYS } = {}) {
  const bounds = windowBounds(now, days);
  let stats;
  try {
    stats = JSON.parse(fs.readFileSync(path.join(profileDir, 'stats-cache.json'), 'utf8'));
  } catch {
    return unavailable({ source: SOURCES.counter.key, reason: 'no CLI stats cache yet' });
  }
  return {
    available: true,
    source: SOURCES.counter.key,
    lifetimeTokens: lifetimeTokens(stats.modelUsage),
    windowTokens: claudeDailyTokens(stats.dailyModelTokens, bounds),
    sessions: Number.isFinite(stats.totalSessions) ? stats.totalSessions : null,
    through: stats.lastComputedDate || null
  };
}

export function codexLogStats(daily, { now = Date.now(), days = WINDOW_DAYS } = {}) {
  const bounds = windowBounds(now, days);
  const dates = (Array.isArray(daily) ? daily : [])
    .map((day) => day?.date)
    .filter((date) => typeof date === 'string')
    .sort();
  return {
    available: true,
    source: SOURCES.logs.key,
    lifetimeTokens: ccusageTokens(daily, null),
    windowTokens: ccusageTokens(daily, bounds),
    sessions: null,
    through: dates.at(-1) || null
  };
}

export async function buildTokenReport({
  poolDir,
  claudeHome,
  codexHome,
  codexProfilesDir,
  now = Date.now(),
  windowDays = WINDOW_DAYS,
  analyzeCodex
} = {}) {
  const discovered = listUsageProfiles({ poolDir, claudeHome, codexHome, codexProfilesDir });
  // Resolve the analyzer once, and only if a Codex profile actually exists, so
  // a machine without Codex never has to have ccusage installed at all.
  let runner;
  const analyze = analyzeCodex || ((profile) => {
    runner ||= resolveCcusageRunner();
    return ccusageDaily({ provider: 'Codex', dir: profile.dir, runner });
  });

  const profiles = [];
  for (const profile of discovered) {
    if (profile.provider === 'Claude') {
      profiles.push({ provider: 'Claude', name: profile.name, ...claudeCliStats(profile.dir, { now, days: windowDays }) });
      continue;
    }
    try {
      profiles.push({
        provider: 'Codex',
        name: profile.name,
        ...codexLogStats(await analyze(profile), { now, days: windowDays })
      });
    } catch (error) {
      profiles.push({
        provider: 'Codex',
        name: profile.name,
        ...unavailable({ source: SOURCES.logs.key, reason: error instanceof Error ? error.message : String(error) })
      });
    }
  }

  const sum = (rows, field) => rows.reduce((total, row) => total + row[field], 0);
  const available = profiles.filter((profile) => profile.available);
  const providers = [...new Set(profiles.map((profile) => profile.provider))].map((provider) => {
    const rows = available.filter((profile) => profile.provider === provider);
    return {
      provider,
      source: profiles.find((profile) => profile.provider === provider).source,
      count: profiles.filter((profile) => profile.provider === provider).length,
      lifetimeTokens: sum(rows, 'lifetimeTokens'),
      windowTokens: sum(rows, 'windowTokens')
    };
  });

  return {
    profiles,
    providers,
    windowDays,
    window: windowBounds(now, windowDays),
    lifetimeTotal: sum(available, 'lifetimeTokens'),
    windowTotal: sum(available, 'windowTokens'),
    complete: available.length === profiles.length
  };
}

const number = (value) => new Intl.NumberFormat('en-US').format(value);

function share(value, total) {
  if (!total) return '';
  return `${bar(value / total)} ${String(Math.round((value / total) * 100)).padStart(3)}%`;
}

function alignedNotes(notes) {
  const width = Math.max(...notes.map(([label]) => label.length));
  return notes.map(([label, detail]) => `  ${label.padEnd(width)}   ${detail}`);
}

export function formatTokenReport(report) {
  if (!report.profiles.length) return 'Token totals\n\n  No Claude or Codex profiles found.';

  const rows = [];
  const rules = [];
  const notes = [];

  for (const [index, group] of report.providers.entries()) {
    if (index > 0) rules.push(rows.length);
    const members = report.profiles.filter((profile) => profile.provider === group.provider);

    for (const [position, profile] of members.entries()) {
      if (!profile.available) {
        notes.push([`${profile.provider} / ${profile.name}`, profile.reason]);
      } else if (profile.through && profile.through < report.window.from) {
        notes.push([`${profile.provider} / ${profile.name}`, `no activity recorded since ${profile.through}`]);
      }

      rows.push([
        position === 0 ? profile.provider : '',
        profile.name,
        profile.available ? number(profile.lifetimeTokens) : '—',
        profile.available ? number(profile.windowTokens) : '—',
        profile.available ? share(profile.windowTokens, report.windowTotal) : ''
      ]);
    }

    // A single-profile provider is already its own subtotal; repeating it is noise.
    if (members.length > 1) {
      rules.push(rows.length);
      rows.push([
        group.provider,
        'subtotal',
        number(group.lifetimeTokens),
        number(group.windowTokens),
        share(group.windowTokens, report.windowTotal)
      ]);
    }
  }

  rules.push(rows.length);
  rows.push([
    'All',
    'TOTAL',
    number(report.lifetimeTotal),
    number(report.windowTotal),
    share(report.windowTotal, report.windowTotal)
  ]);

  const sourceNote = {
    counter: "Claude's own all-time per-model counters",
    logs: 'retained session logs, read by ccusage — a floor, not a ledger'
  };

  return [
    `Token totals · lifetime and the last ${report.windowDays} days`,
    '',
    table({
      headers: ['Provider', 'Profile', 'Lifetime', `Last ${report.windowDays}d`, `${report.windowDays}d share`],
      rows,
      numeric: [2, 3],
      rules
    }),
    '',
    ...report.providers.map((provider) => `${provider.provider}: ${sourceNote[provider.source]}.`),
    `Window ${report.window.from} → ${report.window.to} — the default range on Claude's /stats screen.`,
    'Totals include input, output, and cache tokens.',
    ...(notes.length ? ['', 'Notes', ...alignedNotes(notes)] : [])
  ].join('\n');
}

export async function runTokenReport(options) {
  console.log(await buildTokenReport(options).then(formatTokenReport));
  return 0;
}
