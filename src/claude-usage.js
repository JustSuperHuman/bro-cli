import {
  readClaudeCredentials,
  writeClaudeCredentials
} from './claude-oauth-bridge.js';

const OAUTH_TOKEN_URL = process.env.CLAUDE_OAUTH_TOKEN_URL || 'https://platform.claude.com/v1/oauth/token';
const OAUTH_CLIENT_ID = process.env.CLAUDE_OAUTH_CLIENT_ID || '9d1c250a-e61b-44d9-88ed-5944d1962f5e';
const USAGE_URL = 'https://api.anthropic.com/api/oauth/usage';

export function usageSummary(payload) {
  const fable = Array.isArray(payload?.limits)
    ? payload.limits.find((limit) =>
        limit?.kind === 'weekly_scoped' && limit.scope?.model?.display_name === 'Fable'
      )
    : null;
  return {
    session: payload?.five_hour?.utilization ?? null,
    weekly: payload?.seven_day?.utilization ?? null,
    fable: fable?.percent ?? null
  };
}

async function fetchWithTimeout(url, options, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(timer);
  }
}

async function refreshClaudeToken(configDir, credentials, { timeoutMs }) {
  const oauth = credentials?.claudeAiOauth;
  if (!oauth?.refreshToken) throw new Error('missing OAuth refresh token');
  const response = await fetchWithTimeout(OAUTH_TOKEN_URL, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      grant_type: 'refresh_token',
      refresh_token: oauth.refreshToken,
      client_id: OAUTH_CLIENT_ID
    })
  }, timeoutMs);
  if (!response.ok) throw new Error(`OAuth refresh failed (${response.status})`);
  const body = await response.json();
  if (!body.access_token) throw new Error('OAuth refresh returned no access token');
  oauth.accessToken = body.access_token;
  oauth.refreshToken = body.refresh_token || oauth.refreshToken;
  oauth.expiresAt = Date.now() + Number(body.expires_in || 3600) * 1000;
  if (typeof body.scope === 'string') oauth.scopes = body.scope.split(/\s+/).filter(Boolean);
  writeClaudeCredentials(credentials, { configDir });
  return oauth.accessToken;
}

// Fetch the same subscription meters Claude's account picker presents. Token
// refresh is persisted to the profile's real credential store, so the bridge
// and Claude Code keep seeing one coherent login.
export async function fetchClaudeUsage({ configDir, timeoutMs = 6000 } = {}) {
  const credentials = readClaudeCredentials({ configDir });
  const oauth = credentials?.claudeAiOauth;
  if (!oauth?.accessToken) throw new Error('missing OAuth access token');
  let token = oauth.accessToken;
  if (oauth.expiresAt && oauth.expiresAt <= Date.now()) {
    token = await refreshClaudeToken(configDir, credentials, { timeoutMs });
  }

  const request = (accessToken) => fetchWithTimeout(USAGE_URL, {
    headers: {
      authorization: `Bearer ${accessToken}`,
      'anthropic-version': '2023-06-01',
      'anthropic-beta': 'oauth-2025-04-20',
      accept: 'application/json'
    }
  }, timeoutMs);
  let response = await request(token);
  if (response.status === 401) {
    response = await request(await refreshClaudeToken(configDir, credentials, { timeoutMs }));
  }
  if (!response.ok) throw new Error(`usage request failed (${response.status})`);
  return usageSummary(await response.json());
}
