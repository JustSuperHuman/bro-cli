// Every login bro can launch under, with its usage: the Claude pool accounts
// and the Codex logins side by side, because the Usage section beside the
// logo (banner.js) covers all of them.
//
// Each login is fetched through the shared cache in usage.js, so the Usage
// section, the Claude and Codex panes and the resume / manage menus after
// them cost one fetch per login, not one per menu.

import { accountDirFor, listAccounts } from './claude-accounts.js';
import { claudeHistoryKey, claudeIdentity, fetchClaudeUsage } from './claude-usage.js';
import { fetchCodexUsage } from './codex-usage.js';
import { listCodexProfiles, localCodexProfile } from './codex-profiles.js';
import { appHeadroom, leftLines, leftNotes, peekUsage, requestUsage } from './usage.js';
import { measuredRatio } from './usage-history.js';

// --- Claude ------------------------------------------------------------------

const claudeKey = (account) => `claude:${accountDirFor(account.name)}`;

// Start fetching every signed-in account's meters — or reuse the answers a
// menu opened a moment ago already has. One promise per account, never
// rejecting. The identity lets an account imported under two names count
// once; every fetch also records a reading (claude-usage.js).
export function requestClaudeUsages(accounts) {
  return accounts
    .filter((account) => account.authenticated)
    .map((account) => requestUsage(claudeKey(account), async () => {
      const configDir = accountDirFor(account.name);
      const [stats, identity] = await Promise.all([fetchClaudeUsage({ configDir }), claudeIdentity(configDir)]);
      return { ...stats, identity };
    }));
}

// An account as its row should show it right now: meters once they've
// arrived, placeholders until then.
export function withClaudeUsage(account) {
  if (!account.authenticated) return account;
  const usageStats = peekUsage(claudeKey(account));
  return { ...account, usageStats, usagePending: usageStats === undefined };
}

// --- Codex -------------------------------------------------------------------

// This machine's login first, then each profile.
export const codexLogins = () => [localCodexProfile(), ...listCodexProfiles()];

// Each login's meters come from a short-lived `codex app-server` under that
// login's home. The local login passes no home, so its fetch resolves the
// same credentials a launch would.
const codexHome = (login) => (login.name ? login.dir : '');
const codexKey = (login) => `codex:${codexHome(login)}`;

export function requestCodexUsages(logins) {
  return logins
    .filter((login) => login.authenticated)
    .map((login) => requestUsage(codexKey(login), () => fetchCodexUsage({ home: codexHome(login) })));
}

export function withCodexUsage(login) {
  if (!login.authenticated) return login;
  const usageStats = peekUsage(codexKey(login));
  return { ...login, usageStats, usagePending: usageStats === undefined };
}

// --- both --------------------------------------------------------------------

// Start every login's fetch, Claude and Codex. Returns the logins covered and
// one promise per fetch, for a menu to repaint on.
export function requestAllUsage({ accounts = listAccounts(), logins = codexLogins() } = {}) {
  return {
    accounts,
    logins,
    promises: [...requestClaudeUsages(accounts), ...requestCodexUsages(logins)]
  };
}

// What each app has left right now (usage.js appHeadroom), each Claude
// account's 5-hour figures bounded by what its week still covers wherever
// its 5h-to-week ratio has been measured — by the account, its plan tier, or
// failing both, every plan together.
function usageSummary({ accounts, logins }) {
  const claude = accounts
    .filter((account) => account.authenticated)
    .map(withClaudeUsage)
    .map((account) => {
      const identity = account.usageStats?.identity;
      const measured = account.usageStats
        ? measuredRatio({ key: claudeHistoryKey(accountDirFor(account.name), identity), tier: account.rateLimitTier })
        : null;
      return { stats: account.usageStats, identity, ratio: measured?.ratio ?? null, ratioSource: measured?.source ?? null };
    });
  const codex = logins
    .filter((login) => login.authenticated)
    .map(withCodexUsage)
    .map((login) => ({ stats: login.usageStats, identity: login.identity }));
  return appHeadroom({ claude, codex });
}

// The Usage section's content: a heading line, then a line for Claude and one
// for Codex (each only when it has a signed-in login), as functions of the
// width available — and the notes its figures need.
export function usageLeftSection(usage) {
  const summary = usageSummary(usage);
  return { lines: leftLines(summary), notes: leftNotes(summary) };
}
