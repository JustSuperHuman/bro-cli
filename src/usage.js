// Subscription meters across logins — what the Claude and Codex profile panes
// show beside each login, and the "what's left" summary above them.
//
// Every login's meters cost a network round trip (Claude's usage endpoint, or
// a short-lived `codex app-server`), so no menu waits for them: rows paint at
// once with placeholders and fill in as each login answers. Answers are kept
// for a minute, so the menus that follow a pick (resume destination, manage
// profiles) open with the numbers already in place instead of fetching again.

import { ICONS, ICON_WIDTH } from './icons.js';

const USAGE_TTL_MS = 60 * 1000;
const RESET = '\x1b[0m';
const DIM = '\x1b[2m';
const NORMAL = '\x1b[22m';
const RED = '\x1b[31m';
const AMBER = '\x1b[33m';
const GREEN = '\x1b[32m';

// --- one fetch per login, shared by every menu ------------------------------

const entries = new Map();

// Start (or reuse) one login's fetch. Resolves the meters, or null when they
// couldn't be read — never rejects, so callers can fan out without guards.
export function requestUsage(key, fetcher, { maxAge = USAGE_TTL_MS } = {}) {
  const hit = entries.get(key);
  if (hit && (hit.pending || Date.now() - hit.at < maxAge)) return hit.promise;
  const entry = { at: Date.now(), pending: true, value: undefined, promise: null };
  entry.promise = Promise.resolve()
    .then(fetcher)
    .then((value) => value ?? null, () => null)
    .then((value) => {
      Object.assign(entry, { at: Date.now(), pending: false, value });
      return value;
    });
  entries.set(key, entry);
  return entry.promise;
}

// What is known right now: undefined while the first fetch is in flight (or
// none was started), null when it failed, otherwise the meters.
export function peekUsage(key) {
  const entry = entries.get(key);
  return entry && !entry.pending ? entry.value : undefined;
}

export function clearUsageCache() {
  entries.clear();
}

// A select() `live` hook for menus whose labels read peekUsage(): repaint as
// each fetch lands, and stop once the menu has closed.
export function repaintOnUsage(promises) {
  return (repaint) => {
    let open = true;
    for (const promise of promises) promise.then(() => { if (open) repaint(); });
    return () => { open = false; };
  };
}

// --- per-login meters ---------------------------------------------------------

// A used percentage, coloured by pressure (green → amber → red) so the stats
// read at a glance instead of being one dim blur.
export function usagePercent(value) {
  if (typeof value !== 'number') return `${DIM}—${RESET}`;
  const pct = Math.round(value);
  const color = pct >= 80 ? RED : pct >= 50 ? AMBER : GREEN;
  return `${color}${pct}%${RESET}`;
}

// "5h 17% · wk 28% · Fable 53%" for `meters` given as [[label, used], …].
// Pending meters show a placeholder of the same shape, so a row doesn't
// change its layout when the numbers land.
export function metersText(meters, { pending = false } = {}) {
  return meters
    .map(([label, used]) => `${DIM}${label}${RESET} ${pending ? `${DIM}…${RESET}` : usagePercent(used)}`)
    .join(` ${DIM}·${RESET} `);
}

// Codex reports up to two rolling windows, and which one is which depends on
// the plan (Pro currently carries only the weekly one), so they are placed by
// length rather than by position. A window without a length keeps the
// historical order: primary is the short one.
export function codexMeters(summary) {
  const meters = { session: null, weekly: null };
  for (const [window, fallback] of [[summary?.primary, 'session'], [summary?.secondary, 'weekly']]) {
    if (typeof window?.usedPercent !== 'number') continue;
    const minutes = window.windowDurationMins;
    const slot = typeof minutes === 'number' ? (minutes >= 24 * 60 ? 'weekly' : 'session') : fallback;
    if (meters[slot] == null) meters[slot] = window.usedPercent;
  }
  return meters;
}

// --- what is left ---------------------------------------------------------------

const left = (used) => (typeof used === 'number' && Number.isFinite(used) ? Math.min(100, Math.max(0, 100 - used)) : null);
// An exhausted weekly allowance closes every shorter window along with it.
const gated = (value, weekly) => (value == null ? null : weekly === 0 ? 0 : value);
const tighter = (a, b) => (a == null ? b : b == null ? a : Math.min(a, b));

// One Claude account's headroom, as a percentage of its own allowance per
// window. Only accounts whose usage reports a Fable limit can use Fable at
// all; the rest have no Fable headroom (null), not their overall headroom.
// Where there is one, Fable is capped by the account's overall limits and by
// its own weekly limit on top — whichever is tighter.
export function claudeHeadroom(stats) {
  if (!stats) return null;
  const wk = left(stats.weekly);
  const fableWk = left(stats.fable) == null ? null : tighter(wk, left(stats.fable));
  return {
    h5: gated(left(stats.session), wk),
    wk,
    fable5h: fableWk == null ? null : gated(left(stats.session), fableWk),
    fableWk
  };
}

export function codexHeadroom(summary) {
  if (!summary) return null;
  const { session, weekly } = codexMeters(summary);
  const wk = left(weekly);
  return { h5: gated(left(session), wk), wk };
}

// The headroom of each distinct login that answered. Two logins signed in as
// the same user draw from one allowance, so the second is skipped.
function distinctRooms(logins, headroom) {
  const seen = new Set();
  const rooms = [];
  for (const { stats, identity } of logins) {
    if (!stats) continue;
    if (identity) {
      if (seen.has(identity)) continue;
      seen.add(identity);
    }
    rooms.push(headroom(stats));
  }
  return rooms;
}

function sumWindow(rooms, key) {
  let left = 0;
  let count = 0;
  for (const room of rooms) {
    if (room[key] == null) continue;
    left += room[key];
    count++;
  }
  return { left, count };
}

// What each app has left across all of its accounts: per window, one 100%
// shared equally by the app's accounts that have that window, so no figure
// can pass 100%. Fable is Claude's alone and a 100% of its own, shared by the
// accounts that have Fable at all.
//
// `claude` and `codex` are [{ stats, identity }] — stats undefined while
// loading, null when unavailable. Each app comes back with its windows in
// whole percent (null when none of its accounts has that window), whether
// any of its logins is still loading, how many couldn't be read, and how many
// signed-in logins it has at all.
export function appHeadroom({ claude = [], codex = [] }) {
  const summarize = (logins, headroom, windows) => {
    const rooms = distinctRooms(logins, headroom);
    const app = {
      logins: logins.length,
      loading: logins.some((login) => login.stats === undefined),
      unavailable: logins.filter((login) => login.stats === null).length
    };
    for (const key of windows) {
      const { left, count } = sumWindow(rooms, key);
      app[key] = count ? Math.round(left / count) : null;
    }
    return app;
  };
  return {
    claude: summarize(claude, claudeHeadroom, ['h5', 'wk', 'fable5h', 'fableWk']),
    codex: summarize(codex, codexHeadroom, ['h5', 'wk'])
  };
}

// A figure, right-aligned in four columns and coloured by how much is left.
// It sets normal intensity itself: a menu paints its header rows dim, and a
// line that starts with an icon has no reset before its first figure.
function leftFigure(value, loading) {
  if (loading) return `${DIM}   …${RESET}`;
  if (value == null) return `${DIM}   —${RESET}`;
  const color = value <= 20 ? RED : value <= 50 ? AMBER : GREEN;
  return `${NORMAL}${color}${String(value).padStart(3)}%${RESET}`;
}

const heading = (text) => `${DIM}${text.padStart(4)}${RESET}`;

// The summary above a profile list: a dim heading line, then one line per app
// with at least one signed-in login — its icon, what's left of its 5h window
// and of its week, and on Claude's line the same two for Fable:
//
//             5h   week              5h   week
//     ✻      74%    55%    Fable    96%     2%
//     >_       —    16%
//
// (the icons shown as the text marks terminals without images get). Each line
// is a function of the width available, like any picker label.
export function leftLines(summary) {
  const { claude, codex } = summary;
  const gap = (n) => ' '.repeat(n);
  const apps = [
    claude.logins && { icon: ICONS.claude, app: claude, fable: true },
    codex.logins && { icon: ICONS.codex, app: codex, fable: false }
  ].filter(Boolean);
  if (!apps.length) return [];
  const withFable = apps.some((row) => row.fable);

  const header = `${gap(ICON_WIDTH + 3)}${heading('5h')}${gap(3)}${heading('week')}`
    + (withFable ? `${gap(4 + 5 + 3)}${heading('5h')}${gap(3)}${heading('week')}` : '');
  const lines = apps.map(({ icon, app, fable }) => {
    let line = `${icon}${gap(3)}${leftFigure(app.h5, app.loading)}${gap(3)}${leftFigure(app.wk, app.loading)}`;
    if (fable) line += `${gap(4)}${DIM}Fable${RESET}${gap(3)}${leftFigure(app.fable5h, app.loading)}${gap(3)}${leftFigure(app.fableWk, app.loading)}`;
    if (app.unavailable) line += `${gap(fable ? 3 : 4)}${DIM}${app.unavailable} unavailable${RESET}`;
    return line;
  });
  return [header, ...lines].map((line) => () => line);
}
