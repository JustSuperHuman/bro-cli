// How much of a week one 5-hour window is worth, measured.
//
// Neither Anthropic nor OpenAI publishes how big a plan's 5-hour window is
// next to its weekly allowance — and it moves (Anthropic doubled the 5-hour
// windows in May 2026 and left the weekly ones alone). Both meters are driven
// by the same usage, though, so it can be read off them: within one 5-hour
// window of one week, the weekly meter climbs a steady fraction of what the
// 5-hour meter climbs. That fraction — weekly points per 5-hour point — is
// what turns "what's left this week" into "how much of this 5-hour window the
// week can still pay for".
//
// Every usage fetch leaves a reading here (~/.bro/usage-history.json), so the
// estimate sharpens with ordinary use of bro and stays current as plans
// change. An account with too little movement of its own borrows the pooled
// estimate of accounts on the same plan tier, or failing that of all plans.

import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';

const HISTORY_FILE = () => process.env.BRO_USAGE_HISTORY || path.join(BRO_DIR, 'usage-history.json');
const KEEP_MS = 14 * 24 * 60 * 60 * 1000;
const KEEP_READINGS = 400;

// The meters report whole percentages, so a 5-hour climb of a few points
// could hide a weekly climb anywhere from zero to twice the true value. Below
// this much 5-hour movement in total there's no estimate at all.
export const MIN_SESSION_POINTS = 20;

// A window's reset time as a whole minute: the API stamps it with the time of
// the request down to the microsecond, so the same window can come back a
// second either side of its boundary.
export function windowId(iso) {
  const at = Date.parse(iso || '');
  return Number.isFinite(at) ? Math.round(at / 60000) : null;
}

// How far the 5-hour and weekly meters climbed together across `readings`
// ([{ at, s, w, sr, wr }]: 5h and weekly used %, and their window ids). Only
// consecutive readings of the same week count, and of the same 5-hour window
// — or a reading before any window was open (0%, no reset time) followed by
// the window that opened.
export function meterMovement(readings) {
  const sorted = [...readings].sort((a, b) => a.at - b.at);
  let session = 0;
  let weekly = 0;
  for (let i = 1; i < sorted.length; i++) {
    const before = sorted[i - 1];
    const after = sorted[i];
    if (before.wr == null || before.wr !== after.wr) continue;
    const sameWindow = before.sr === after.sr || (before.sr == null && before.s === 0);
    if (!sameWindow || after.sr == null) continue;
    const ds = after.s - before.s;
    const dw = after.w - before.w;
    if (ds < 0 || dw < 0) continue;
    session += ds;
    weekly += dw;
  }
  return { session, weekly };
}

// Weekly points per 5-hour point, or null without enough movement to tell.
export function ratioFromMovement({ session, weekly }) {
  return session >= MIN_SESSION_POINTS ? { ratio: weekly / session, sessionPoints: session } : null;
}

let cache = null;

function load(file) {
  if (cache?.file === file) return cache.data;
  let data = { version: 1, logins: {} };
  try {
    const parsed = JSON.parse(fs.readFileSync(file, 'utf8'));
    if (parsed?.logins && typeof parsed.logins === 'object') data = parsed;
  } catch {
    /* no history yet */
  }
  cache = { file, data };
  return data;
}

// Remember one reading of a login's meters. `key` names the account (its
// identity where known, so a profile signed into another account starts a
// new history); `tier` is its plan tier, for pooling. Unchanged meters add
// nothing. Failures to write are ignored — the history is a refinement.
export function recordReading(key, { at = Date.now(), session, weekly, fable = null, sessionResetsAt, weeklyResetsAt, tier = null }, { file = HISTORY_FILE() } = {}) {
  if (!key || typeof session !== 'number' || typeof weekly !== 'number') return;
  const data = load(file);
  const login = (data.logins[key] ||= { tier, readings: [] });
  login.tier = tier || login.tier;
  const reading = { at, s: session, w: weekly, f: fable, sr: windowId(sessionResetsAt), wr: windowId(weeklyResetsAt) };
  const last = login.readings[login.readings.length - 1];
  if (last && last.s === reading.s && last.w === reading.w && last.f === reading.f && last.sr === reading.sr && last.wr === reading.wr) return;
  login.readings = [...login.readings, reading].filter((r) => at - r.at <= KEEP_MS).slice(-KEEP_READINGS);
  try {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const temp = `${file}.${process.pid}.tmp`;
    fs.writeFileSync(temp, JSON.stringify(data));
    fs.renameSync(temp, file);
  } catch {
    /* read-only home, full disk: carry on without */
  }
}

// The movement of every login `include` accepts, added together.
function pooledMovement(logins, include) {
  const total = { session: 0, weekly: 0 };
  for (const login of Object.values(logins)) {
    if (!include(login)) continue;
    const moved = meterMovement(login.readings);
    total.session += moved.session;
    total.weekly += moved.weekly;
  }
  return total;
}

// The measured ratio for a login: its own when it has moved enough; else
// every login on its plan tier added together; else every login on any plan
// — the plans measured so far put a 5-hour window at a similar share of the
// week, so another plan's measure beats none, and the caller is told
// (`source`) so it can say where the figure came from. Returns
// { ratio, sessionPoints, source: 'account' | 'tier' | 'plans' } or null.
export function measuredRatio({ key, tier = null }, { file = HISTORY_FILE() } = {}) {
  const logins = load(file).logins;
  const own = key && logins[key] ? ratioFromMovement(meterMovement(logins[key].readings)) : null;
  if (own) return { ...own, source: 'account' };
  const sameTier = tier ? ratioFromMovement(pooledMovement(logins, (login) => login.tier === tier)) : null;
  if (sameTier) return { ...sameTier, source: 'tier' };
  const anyPlan = ratioFromMovement(pooledMovement(logins, () => true));
  return anyPlan ? { ...anyPlan, source: 'plans' } : null;
}

export function resetHistoryCache() {
  cache = null;
}
