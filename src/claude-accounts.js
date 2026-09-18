// The Claude account profiles bro pools — one CLAUDE_CONFIG_DIR per account
// under ~/.claude-max-pool/accounts/<name>. Read straight from disk, so the
// launcher (pool.js) and the usage summary both see the same accounts without
// starting the pool server.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const POOL_DIR = process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
export const ACCOUNTS_DIR = path.join(POOL_DIR, 'accounts');

export function listAccounts() {
  let names = [];
  try {
    names = fs
      .readdirSync(ACCOUNTS_DIR, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name)
      .sort();
  } catch {
    return [];
  }
  return names.map((name) => {
    let authenticated = false;
    let subscriptionType = null;
    try {
      const creds = JSON.parse(fs.readFileSync(path.join(ACCOUNTS_DIR, name, '.credentials.json'), 'utf8'));
      const oauth = creds && creds.claudeAiOauth;
      authenticated = Boolean(oauth && oauth.accessToken);
      subscriptionType = (oauth && oauth.subscriptionType) || null;
    } catch {
      /* no creds yet */
    }
    return { name, authenticated, subscriptionType };
  });
}

export function accountDirFor(name) {
  return path.join(ACCOUNTS_DIR, name);
}
