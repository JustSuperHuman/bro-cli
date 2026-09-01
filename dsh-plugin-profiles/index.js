import { fetchClaudeUsage } from '../src/claude-usage.js';
import { fetchCodexUsage } from '../src/codex-usage.js';

export const inject = ['webServer'];
export const PROFILE_ENDPOINT = '/bro-profiles';

const CACHE_MS = 15_000;

function isLoopback(address) {
  const value = String(address || '').toLowerCase();
  return value === '127.0.0.1'
    || value === '::1'
    || value === '::ffff:127.0.0.1';
}

// The host needs a private pointer to each credential store to refresh usage.
// The browser receives only display metadata, never paths, tokens or provider
// bridge passwords.
export function publicProfile(profile) {
  return {
    id: String(profile?.id || ''),
    kind: profile?.kind === 'codex' ? 'codex' : 'claude',
    name: String(profile?.name || ''),
    label: String(profile?.label || profile?.name || ''),
    authenticated: profile?.authenticated === true,
    available: profile?.available === true,
    route: String(profile?.route || ''),
    defaultModel: String(profile?.defaultModel || ''),
    models: Array.isArray(profile?.models) ? profile.models.map(String) : [],
    plan: typeof profile?.plan === 'string' ? profile.plan : null,
    ...(profile?.routeError ? { routeError: true } : {})
  };
}

async function usageFor(profile, { claudeUsage, codexUsage }) {
  if (!profile?.authenticated) return null;
  if (profile.usageSource?.kind === 'claude') {
    return claudeUsage({ configDir: profile.usageSource.configDir });
  }
  if (profile.usageSource?.kind === 'codex') {
    return codexUsage({ home: profile.usageSource.home || '' });
  }
  throw new Error('missing usage source');
}

export function createProfileCatalog(profiles = [], {
  claudeUsage = fetchClaudeUsage,
  codexUsage = fetchCodexUsage,
  cacheMs = CACHE_MS,
  now = () => Date.now()
} = {}) {
  const configured = Array.isArray(profiles) ? profiles : [];
  let cachedAt = 0;
  let cached = null;
  let loading = null;

  const load = async ({ refresh = false } = {}) => {
    if (!refresh && cached && now() - cachedAt < cacheMs) return cached;
    if (loading) return loading;
    loading = Promise.all(configured.map(async (profile) => {
      const display = publicProfile(profile);
      if (!profile?.authenticated) return { ...display, usage: null };
      try {
        return { ...display, usage: await usageFor(profile, { claudeUsage, codexUsage }) };
      } catch {
        return { ...display, usage: null, usageError: true };
      }
    })).then((rows) => {
      cached = { profiles: rows, refreshedAt: new Date(now()).toISOString() };
      cachedAt = now();
      return cached;
    }).finally(() => {
      loading = null;
    });
    return loading;
  };

  return { load };
}

export function apply(ctx, config = {}) {
  const catalog = createProfileCatalog(config.profiles);
  ctx.effect(() => ctx.webServer.register({
    kind: 'exact',
    path: PROFILE_ENDPOINT,
    handler: async (req, res) => {
      if (!isLoopback(req.socket?.remoteAddress)) {
        res.writeHead(403, { 'content-type': 'text/plain; charset=utf-8' });
        res.end('forbidden');
        return;
      }
      if (req.method !== 'GET' && req.method !== 'HEAD') {
        res.writeHead(405, { allow: 'GET, HEAD' });
        res.end();
        return;
      }
      try {
        const url = new URL(req.url || PROFILE_ENDPOINT, 'http://127.0.0.1');
        const body = JSON.stringify(await catalog.load({ refresh: url.searchParams.get('refresh') === '1' }));
        res.writeHead(200, {
          'content-type': 'application/json; charset=utf-8',
          'cache-control': 'no-store'
        });
        res.end(req.method === 'HEAD' ? undefined : body);
      } catch {
        res.writeHead(500, {
          'content-type': 'application/json; charset=utf-8',
          'cache-control': 'no-store'
        });
        res.end(JSON.stringify({ error: 'profile catalog unavailable' }));
      }
    }
  }), 'bro profiles: local account catalog');
}
