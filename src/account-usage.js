// Every login bro can launch under, with its usage: the Claude pool accounts
// and the Codex logins side by side, because the Usage section beside the
// logo (banner.js) covers all of them.
//
// Each login is fetched through the shared cache in usage.js, so the Usage
// section, the Claude and Codex panes and the resume / manage menus after
// them cost one fetch per login, not one per menu.

import fs from 'node:fs';
import path from 'node:path';
import { accountDirFor, listAccounts } from './claude-accounts.js';
import { fetchClaudeUsage } from './claude-usage.js';
import { fetchCodexUsage } from './codex-usage.js';
import { listCodexProfiles, localCodexProfile } from './codex-profiles.js';
import { appHeadroom, leftLines, peekUsage, requestUsage } from './usage.js';

// --- Claude ------------------------------------------------------------------

const claudeKey = (account) => `claude:${accountDirFor(account.name)}`;

// Which Claude user (in which organization) a profile is signed in as, so an
// account imported under two names counts once.
async function claudeIdentity(configDir) {
  try {
    const { oauthAccount } = JSON.parse(await fs.promises.readFile(path.join(configDir, '.claude.json'), 'utf8'));
    return oauthAccount?.accountUuid ? `${oauthAccount.accountUuid}:${oauthAccount.organizationUuid || ''}` : null;
  } catch {
    return null;
  }
}

// Start fetching every signed-in account's meters — or reuse the answers a
// menu opened a moment ago already has. One promise per account, never
// rejecting.
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

// What each app has left right now: a heading line, then a line for Claude
// and one for Codex (each only when it has a signed-in login), as functions
// of the width available.
export function usageLeftLines({ accounts, logins }) {
  const claude = accounts
    .filter((account) => account.authenticated)
    .map(withClaudeUsage)
    .map((account) => ({ stats: account.usageStats, identity: account.usageStats?.identity }));
  const codex = logins
    .filter((login) => login.authenticated)
    .map(withCodexUsage)
    .map((login) => ({ stats: login.usageStats, identity: login.identity }));
  return leftLines(appHeadroom({ claude, codex }));
}
