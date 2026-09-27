// Picking a login by what it has left, instead of by name.
//
//   bro --large-task    the roomiest login: a long job shouldn't hit a wall
//                       halfway through because it landed on a near-spent
//                       account.
//   bro --small-task    the emptiest login that can still finish: small work
//                       burns down the accounts that are nearly done for the
//                       week anyway, and leaves the roomy ones roomy.
//
// Both read the same meters the Usage banner reads (usage.js), through the
// same per-login cache — so choosing a login costs one round trip per login,
// shared with whatever menu opens afterwards, and the figures here always
// agree with the ones the banner shows.
//
// --- ranking on both meters ------------------------------------------------
//
// A login's two meters measure different things, and either one alone ranks
// the pool wrongly: an account with a wide-open 5-hour window and one weekly
// point left is nearly finished, and an account with a full week but a spent
// window cannot start anything for the next few hours. So both decide, in
// one currency:
//
//   week   what is left of the allowance at all, in weekly points. This is
//          the budget — a week refills once, a 5-hour window five times a
//          day — so it ranks the logins.
//   now    what can be spent before this 5-hour window resets, converted
//          into those same weekly points through the account's measured
//          5h-to-week ratio (usage-history.js): min(5h x ratio, week). This
//          is the rate, so it gates — a login spent in either window is
//          passed over — and it breaks ties between equal weeks.
//
// --large-task takes the highest pair, --small-task the lowest.

import { accountDirFor, listAccounts } from './claude-accounts.js';
import { claudeHistoryKey } from './claude-usage.js';
import {
  codexLogins,
  requestClaudeUsages,
  requestCodexUsages,
  withClaudeUsage,
  withCodexUsage
} from './account-usage.js';
import { claudeHeadroom, codexHeadroom } from './usage.js';
import { measuredRatio } from './usage-history.js';

export const TASK_SIZES = ['large', 'small'];

// Below this much of a window a login can't be counted on to finish
// anything: its week is all but gone, or its 5-hour window is. Whole percent
// of that window's own allowance, the same unit the Usage banner prints.
export const READY_FLOOR = 5;

const APPS = {
  claude: { label: 'Claude' },
  codex: { label: 'Codex' }
};

const number = (value) => (typeof value === 'number' && Number.isFinite(value) ? value : null);

// What a login can still deliver, as { week, now } in weekly points — null
// when neither meter could be read.
//
// `ratio` is the account's measured weekly points per 5-hour point. Without
// one the two meters can't be converted into each other, so `now` falls back
// to the tighter of the two percentages: a bound that is still right about
// which login is the more spent, just coarser about by how much.
export function capacityOf(room, { ratio = null } = {}) {
  if (!room) return null;
  const week = number(room.wk);
  const window = number(room.h5);
  if (week == null && window == null) return null;
  // A login reporting only one window is judged on the one it reports —
  // Codex's Pro plan carries no 5-hour meter at all.
  if (week == null) return { week: window, now: window };
  if (window == null) return { week, now: week };
  // claudeHeadroom has already capped the 5-hour figure by what the week can
  // pay for, so the conversion lands at or below the week by construction;
  // min() keeps that true for the unmeasured fallback too.
  return { week, now: Math.min(ratio > 0 ? window * ratio : window, week) };
}

// Whether a login can be relied on to finish a job: every meter it reports
// has to be above the floor. A spent 5-hour window blocks the job now; a
// spent week blocks it for days.
export function isReady(room) {
  if (!room) return false;
  for (const value of [number(room.wk), number(room.h5)]) {
    if (value != null && value < READY_FLOOR) return false;
  }
  return true;
}

// `candidates` are [{ name, label, room, ratio }] as the two builders below
// return them; `size` is 'large' or 'small'. Returns the chosen candidate
// with its capacity and whether it was ready, or null when not one of them
// could be read.
//
// The week ranks, the 5-hour window breaks ties, and the name settles it
// last so an untouched pool ranks the same way twice running.
export function pickBySize(candidates, size) {
  const rated = candidates
    .map((candidate) => ({
      ...candidate,
      capacity: capacityOf(candidate.room, { ratio: candidate.ratio }),
      ready: isReady(candidate.room)
    }))
    .filter((candidate) => candidate.capacity);
  if (!rated.length) return null;
  // Every login is spent somewhere: none can be relied on, so rank the whole
  // field rather than pretend there is a usable one.
  const ready = rated.filter((candidate) => candidate.ready);
  const direction = size === 'small' ? 1 : -1;
  return [...(ready.length ? ready : rated)].sort((a, b) =>
    direction * (a.capacity.week - b.capacity.week)
    || direction * (a.capacity.now - b.capacity.now)
    || String(a.name).localeCompare(String(b.name))
  )[0];
}

// Every signed-in Claude pool account with its headroom and its measured
// 5h-to-week ratio. The 5-hour figure claudeHeadroom returns is already
// capped by what the week can pay for wherever that ratio is known — the
// same correction the banner applies, so the login this picks is the one the
// banner's figures point at.
export async function claudeCandidates() {
  const accounts = listAccounts().filter((account) => account.authenticated);
  await Promise.all(requestClaudeUsages(accounts));
  return accounts.map((account) => {
    const stats = withClaudeUsage(account).usageStats || null;
    const measured = stats
      ? measuredRatio({
          key: claudeHistoryKey(accountDirFor(account.name), stats.identity),
          tier: account.rateLimitTier
        })
      : null;
    const ratio = measured?.ratio ?? null;
    return {
      name: account.name,
      label: account.name,
      ratio,
      room: claudeHeadroom(stats, { ratio })
    };
  });
}

// Every signed-in Codex login with its headroom — this machine's own first,
// which is the one whose name is the empty string everywhere else in bro.
// Codex keeps no reading history, so its windows are compared unconverted.
export async function codexCandidates() {
  const logins = codexLogins().filter((login) => login.authenticated);
  await Promise.all(requestCodexUsages(logins));
  return logins.map((login) => ({
    name: login.name,
    label: login.name || "this machine's Codex login",
    ratio: null,
    room: codexHeadroom(withCodexUsage(login).usageStats || null)
  }));
}

// Which login `--large-task` / `--small-task` means for one app.
//
// Resolves to { name, label, room, capacity, spent, candidates } — `name` is
// what --account takes, so '' is Codex's local login, not "none". When no
// login's meters could be read it resolves to { name: null, reason }: the
// caller falls back to asking rather than guessing, because a guess here
// spends the wrong subscription.
export async function resolveTaskProfile({ app = 'claude', size = 'large', candidates } = {}) {
  const { label } = APPS[app] || APPS.claude;
  const list = candidates || (app === 'codex' ? await codexCandidates() : await claudeCandidates());
  if (!list.length) return { name: null, reason: `No signed-in ${label} login to choose from.`, candidates: list };
  const chosen = pickBySize(list, size);
  if (!chosen) {
    return { name: null, reason: `Couldn't read usage for any ${label} login just now.`, candidates: list };
  }
  return {
    name: chosen.name,
    label: chosen.label,
    room: chosen.room,
    capacity: chosen.capacity,
    // Nothing passed the floor: every login is spent in one window or the
    // other, and this one is only the best of them.
    spent: !chosen.ready,
    candidates: list
  };
}

// What a login has left, in the words its meters support: both windows when
// both were reported, otherwise the one that was.
export function roomText(room) {
  const week = number(room?.wk) == null ? '' : `week ${Math.round(room.wk)}%`;
  const session = number(room?.h5) == null ? '' : `5h ${Math.round(room.h5)}%`;
  return `${[week, session].filter(Boolean).join(' · ') || '—'} left`;
}

// The one line bro prints about its choice, on stderr like every other
// diagnostic (out.js).
export function describeTaskChoice(chosen, size) {
  const name = chosen.label || chosen.name || 'this machine';
  const room = roomText(chosen.room);
  if (chosen.spent) {
    return `${size === 'small' ? 'Small' : 'Large'} task: every login is spent somewhere — using ${name} (${room}).`;
  }
  return size === 'small'
    ? `Small task: ${name} — the emptiest login that can still finish it (${room}).`
    : `Large task: ${name} — the roomiest login (${room}).`;
}
