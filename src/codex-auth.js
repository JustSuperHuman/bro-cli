// ChatGPT-subscription OAuth for the Codex backend — self-contained, no codex
// CLI required. Implements the same PKCE login flow the Codex CLI uses
// (auth.openai.com, localhost:1455 callback), refreshes tokens itself, and
// stores credentials at ~/.bro/codex-auth.json in Codex's own auth.json format.
// If the Codex CLI *is* installed and logged in, its ~/.codex/auth.json is
// reused automatically instead of asking the user to sign in again.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { BRO_DIR } from './config.js';

const ISSUER = 'https://auth.openai.com';
const CLIENT_ID = 'app_EMoamEEZ73f0CkXaXp7hrann'; // the Codex CLI's public OAuth client
const CALLBACK_PORT = 1455; // must match the client's registered redirect URI
const REDIRECT_URI = `http://localhost:${CALLBACK_PORT}/auth/callback`;

export const BRO_AUTH_PATH = path.join(BRO_DIR, 'codex-auth.json');
const CODEX_HOME = process.env.CODEX_HOME || path.join(os.homedir(), '.codex');
const CODEX_CLI_AUTH_PATH = path.join(CODEX_HOME, 'auth.json');

function readJson(p) {
  try {
    return JSON.parse(fs.readFileSync(p, 'utf8'));
  } catch {
    return null;
  }
}

function jwtPayload(token) {
  try {
    return JSON.parse(Buffer.from(String(token).split('.')[1], 'base64url').toString('utf8'));
  } catch {
    return null;
  }
}

function accountIdFromTokens(tokens) {
  if (tokens.account_id) return tokens.account_id;
  for (const t of [tokens.id_token, tokens.access_token]) {
    const claim = jwtPayload(t)?.['https://api.openai.com/auth'];
    if (claim?.chatgpt_account_id) return claim.chatgpt_account_id;
  }
  return null;
}

// Load stored credentials: bro's own file first, then the Codex CLI's.
// Returns { accessToken, refreshToken, accountId, path } or null.
export function loadCodexAuth() {
  for (const p of [BRO_AUTH_PATH, CODEX_CLI_AUTH_PATH]) {
    const tokens = readJson(p)?.tokens;
    if (tokens?.access_token && tokens?.refresh_token) {
      return {
        accessToken: tokens.access_token,
        refreshToken: tokens.refresh_token,
        accountId: accountIdFromTokens(tokens),
        path: p
      };
    }
  }
  return null;
}

export function isCodexLoggedIn() {
  return loadCodexAuth() != null;
}

// Persist tokens back to whichever file they came from, preserving the Codex
// auth.json shape so the Codex CLI (when present) keeps working too.
function saveTokens(file, tokens) {
  const existing = readJson(file) || { OPENAI_API_KEY: null };
  existing.auth_mode = 'chatgpt';
  existing.tokens = { ...(existing.tokens || {}), ...tokens };
  existing.last_refresh = new Date().toISOString();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(existing, null, 2));
}

async function refreshTokens(auth) {
  const res = await fetch(`${ISSUER}/oauth/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      client_id: CLIENT_ID,
      grant_type: 'refresh_token',
      refresh_token: auth.refreshToken,
      scope: 'openid profile email'
    })
  });
  if (!res.ok) throw new Error(`Codex token refresh failed (HTTP ${res.status}). Run: bro codex login`);
  const body = await res.json();
  if (!body.access_token) throw new Error('Codex token refresh returned no access token.');
  saveTokens(auth.path, {
    access_token: body.access_token,
    ...(body.refresh_token ? { refresh_token: body.refresh_token } : {}),
    ...(body.id_token ? { id_token: body.id_token } : {})
  });
  return loadCodexAuth();
}

// Return credentials with a non-expired access token, refreshing (and
// persisting) when it expires within the next minute. `force` skips the
// expiry check — used after an upstream 401.
export async function freshCodexAuth({ force = false } = {}) {
  let auth = loadCodexAuth();
  if (!auth) throw new Error('Not logged in to a ChatGPT subscription. Run: bro codex login');
  const exp = jwtPayload(auth.accessToken)?.exp;
  if (force || (exp && exp * 1000 < Date.now() + 60_000)) auth = await refreshTokens(auth);
  return auth;
}

export function codexLogout() {
  let removed = false;
  try {
    fs.unlinkSync(BRO_AUTH_PATH);
    removed = true;
  } catch {
    /* nothing stored */
  }
  return removed;
}

function openBrowser(url) {
  try {
    if (process.platform === 'win32') {
      // rundll32 handles URLs with & safely, unlike `cmd /c start`.
      spawn('rundll32', ['url.dll,FileProtocolHandler', url], { stdio: 'ignore', detached: true }).unref();
    } else {
      spawn(process.platform === 'darwin' ? 'open' : 'xdg-open', [url], { stdio: 'ignore', detached: true }).unref();
    }
  } catch {
    /* user can copy the printed URL */
  }
}

// Interactive PKCE login: starts the localhost callback server, opens the
// browser sign-in, exchanges the code, and stores tokens at BRO_AUTH_PATH.
export async function codexLogin({ timeoutMs = 10 * 60 * 1000 } = {}) {
  const verifier = crypto.randomBytes(64).toString('base64url');
  const challenge = crypto.createHash('sha256').update(verifier).digest('base64url');
  const state = crypto.randomBytes(32).toString('base64url');

  const authorizeUrl =
    `${ISSUER}/oauth/authorize?` +
    new URLSearchParams({
      response_type: 'code',
      client_id: CLIENT_ID,
      redirect_uri: REDIRECT_URI,
      scope: 'openid profile email offline_access',
      code_challenge: challenge,
      code_challenge_method: 'S256',
      id_token_add_organizations: 'true',
      codex_cli_simplified_flow: 'true',
      originator: 'codex_cli_rs',
      state
    }).toString();

  const code = await new Promise((resolve, reject) => {
    const server = http.createServer((req, res) => {
      const url = new URL(req.url, `http://localhost:${CALLBACK_PORT}`);
      if (url.pathname !== '/auth/callback') {
        res.writeHead(404).end();
        return;
      }
      const err = url.searchParams.get('error');
      const gotState = url.searchParams.get('state');
      const gotCode = url.searchParams.get('code');
      const ok = !err && gotCode && gotState === state;
      res.writeHead(ok ? 200 : 400, { 'content-type': 'text/html' });
      res.end(
        ok
          ? '<h2>Signed in — you can close this tab and return to the terminal.</h2>'
          : `<h2>Sign-in failed${err ? `: ${err}` : ''}. Return to the terminal and retry.</h2>`
      );
      server.close();
      if (ok) resolve(gotCode);
      else reject(new Error(err || 'state mismatch in OAuth callback'));
    });
    server.on('error', (e) =>
      reject(
        e.code === 'EADDRINUSE'
          ? new Error(`Port ${CALLBACK_PORT} is in use (is a codex login already running?). Close it and retry.`)
          : e
      )
    );
    const timer = setTimeout(() => {
      server.close();
      reject(new Error('Login timed out.'));
    }, timeoutMs);
    timer.unref?.();
    server.listen(CALLBACK_PORT, '127.0.0.1', () => {
      console.log('\nOpening the ChatGPT sign-in in your browser…');
      console.log(`If it does not open, visit:\n  ${authorizeUrl}\n`);
      openBrowser(authorizeUrl);
    });
  });

  const res = await fetch(`${ISSUER}/oauth/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'authorization_code',
      code,
      redirect_uri: REDIRECT_URI,
      client_id: CLIENT_ID,
      code_verifier: verifier
    }).toString()
  });
  if (!res.ok) throw new Error(`Token exchange failed (HTTP ${res.status}): ${(await res.text()).slice(0, 300)}`);
  const body = await res.json();
  const tokens = {
    id_token: body.id_token,
    access_token: body.access_token,
    refresh_token: body.refresh_token
  };
  tokens.account_id = accountIdFromTokens(tokens);
  saveTokens(BRO_AUTH_PATH, tokens);

  const plan = jwtPayload(body.id_token)?.['https://api.openai.com/auth']?.chatgpt_plan_type;
  console.log(`Logged in to ChatGPT${plan ? ` (${plan} plan)` : ''}. Credentials saved to ${BRO_AUTH_PATH}`);
  return loadCodexAuth();
}

export function codexAuthStatus() {
  const auth = loadCodexAuth();
  if (!auth) return { loggedIn: false };
  const claim = jwtPayload(auth.accessToken)?.['https://api.openai.com/auth'] || {};
  return {
    loggedIn: true,
    source: auth.path,
    plan: claim.chatgpt_plan_type || null,
    accountId: auth.accountId
  };
}
