// A single-login form of bro's tested Claude account-pool transport.
//
// DSH's pi-ai adapter understands Claude OAuth request semantics, but DSH does
// not read Claude Code's credential store. This loopback bridge points the pool
// server at the active CLAUDE_CONFIG_DIR directly, so refresh-token rotation is
// written back to the real login instead of a disposable credential copy.

import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { ensureBun } from './proc.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const POOL_ENTRY = path.join(__dirname, '..', 'pool', 'src', 'index.ts');
const DIRECT_ACCOUNT = 'claude-code-login';

const concreteModels = (models = []) => models.filter((model) => model?.id);

function macKeychainCredentials() {
  if (process.platform !== 'darwin') return null;
  try {
    const result = spawnSync('security', [
      'find-generic-password', '-s', 'Claude Code-credentials', '-w'
    ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });
    if (result.status !== 0) return null;
    return JSON.parse(String(result.stdout || '').trim());
  } catch {
    return null;
  }
}

function writeMacKeychainCredentials(credentials) {
  if (process.platform !== 'darwin') return false;
  try {
    const result = spawnSync('security', [
      'add-generic-password', '-U', '-s', 'Claude Code-credentials', '-w', JSON.stringify(credentials)
    ], { encoding: 'utf8', stdio: ['ignore', 'ignore', 'ignore'] });
    return result.status === 0;
  } catch {
    return false;
  }
}

// Read and write the credential store Claude Code itself owns. macOS keeps the
// default login in Keychain; profile directories and the other platforms use
// .credentials.json. Callers receive only the parsed document and never need
// to copy an OAuth token into another store.
export function readClaudeCredentials({
  configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude')
} = {}) {
  const credentialsPath = path.join(configDir, '.credentials.json');
  try {
    return JSON.parse(fs.readFileSync(credentialsPath, 'utf8'));
  } catch {
    return macKeychainCredentials();
  }
}

export function writeClaudeCredentials(credentials, {
  configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude')
} = {}) {
  const credentialsPath = path.join(configDir, '.credentials.json');
  // A default macOS login that came from Keychain must rotate back into that
  // same item. Profile logins already have a file and stay file-backed.
  if (process.platform === 'darwin' && !fs.existsSync(credentialsPath)) {
    if (!writeMacKeychainCredentials(credentials)) {
      throw new Error('Could not update Claude Code credentials in the macOS Keychain');
    }
    return;
  }
  fs.mkdirSync(configDir, { recursive: true });
  fs.writeFileSync(credentialsPath, JSON.stringify(credentials, null, 2), { mode: 0o600 });
}

export function claudeCodeLoginStatus({
  configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude')
} = {}) {
  const credentials = readClaudeCredentials({ configDir });
  const oauth = credentials?.claudeAiOauth;
  return {
    configDir,
    authenticated: Boolean(oauth?.accessToken),
    subscriptionType: oauth?.subscriptionType || null,
    expiresAt: Number.isFinite(oauth?.expiresAt) ? oauth.expiresAt : null
  };
}

export function claudeOAuthBridgeProvider(provider, { baseUrl, models = [] }) {
  const available = concreteModels(models);
  const fallback = concreteModels(provider?.models);
  return {
    ...provider,
    mode: 'anthropic',
    baseUrl,
    noKey: false,
    disable1mContext: true,
    // This route name lets llm-pi-ai inherit its current Anthropic model
    // metadata (thinking modes, context windows, modalities, token limits).
    dshRoute: 'anthropic',
    models: available.length ? available : fallback
  };
}

function reservePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      server.close((error) => error ? reject(error) : resolve(port));
    });
  });
}

async function fetchJson(url, { timeoutMs = 3000 } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      signal: controller.signal,
      headers: { accept: 'application/json', connection: 'close' }
    });
    if (!response.ok) return null;
    return await response.json();
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

async function waitHealthy(baseUrl, failed, timeoutMs = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (failed()) return false;
    const health = await fetchJson(`${baseUrl}/health`, { timeoutMs: 1200 });
    if (health?.status === 'ok') return true;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  return false;
}

export async function startClaudeOAuthBridge({
  configDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude'),
  port = Number.parseInt(process.env.BRO_DSH_CLAUDE_PORT || '', 10) || 0,
  ensureRuntime = ensureBun,
  spawnProcess = spawn
} = {}) {
  const login = claudeCodeLoginStatus({ configDir });
  if (!login.authenticated) {
    throw new Error(
      `No Claude Code OAuth login was found in ${configDir}. Run \`claude\`, complete /login, then retry DSH.`
    );
  }

  const { bun, dirs } = ensureRuntime();
  const poolDir = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-dsh-claude-'));
  const listenPort = port || await reservePort();
  const baseUrl = `http://127.0.0.1:${listenPort}`;
  // pi-ai detects the sk-ant-oat marker and emits Claude Code's OAuth identity
  // request shape. This is only a random loopback password; the real OAuth
  // credential remains inside the pool server and Claude credential store.
  const apiKey = `sk-ant-oat-bro-${randomBytes(24).toString('hex')}`;
  const env = {
    ...process.env,
    CLAUDE_POOL_DIR: poolDir,
    CLAUDE_DIRECT_CONFIG_DIR: configDir,
    CLAUDE_DIRECT_ACCOUNT_NAME: DIRECT_ACCOUNT,
    CLAUDE_POOL_BACKEND: 'oauth',
    PROXY_API_KEY: apiKey,
    HOST: '127.0.0.1',
    PORT: String(listenPort),
    PATH: [...dirs, process.env.PATH || ''].join(path.delimiter)
  };

  let child = null;
  let stopping = false;
  let restartTimer = null;
  let permanentFailure = '';
  let logs = '';
  let fastExits = 0;

  const consume = (chunk) => {
    logs = `${logs}${chunk}`.slice(-8192);
  };
  const spawnServer = () => {
    const startedAt = Date.now();
    const next = spawnProcess(bun, ['run', POOL_ENTRY, 'serve'], {
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true
    });
    child = next;
    next.stdout?.on('data', consume);
    next.stderr?.on('data', consume);
    next.on('error', (error) => {
      consume(error.message);
      permanentFailure = error.message;
    });
    next.on('exit', (code, signal) => {
      if (child === next) child = null;
      if (stopping) return;
      fastExits = Date.now() - startedAt < 5000 ? fastExits + 1 : 0;
      if (fastExits >= 3) {
        permanentFailure = `Claude OAuth bridge repeatedly exited (${signal || `code ${code}`})`;
        return;
      }
      restartTimer = setTimeout(spawnServer, 500);
      restartTimer.unref?.();
    });
  };

  const stop = async () => {
    if (stopping) return;
    stopping = true;
    if (restartTimer) clearTimeout(restartTimer);
    const active = child;
    if (active && active.exitCode === null) {
      const exited = new Promise((resolve) => active.once('exit', resolve));
      try { active.kill(); } catch {}
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 2000))]);
    }
    fs.rmSync(poolDir, { recursive: true, force: true });
  };

  try {
    spawnServer();
    const healthy = await waitHealthy(baseUrl, () => Boolean(permanentFailure));
    if (!healthy) {
      throw new Error(
        `Claude OAuth bridge did not become healthy on ${baseUrl}` +
          (permanentFailure || logs ? `: ${permanentFailure || logs.trim()}` : '')
      );
    }
    const status = await fetchJson(`${baseUrl}/api/status`);
    const account = status?.accounts?.find((entry) => entry.name === DIRECT_ACCOUNT);
    if (!account?.authenticated) {
      throw new Error(`Claude Code OAuth credentials in ${configDir} could not be loaded by the bridge.`);
    }
    const modelResponse = await fetchJson(`${baseUrl}/v1/models`, { timeoutMs: 12000 });
    const models = (modelResponse?.data || [])
      .filter((entry) => entry?.id)
      .map((entry) => ({ id: entry.id, name: entry.display_name || entry.id }));
    return {
      baseUrl,
      apiKey,
      models,
      configDir,
      account: {
        name: account.name,
        subscriptionType: account.subscriptionType || login.subscriptionType || null
      },
      stop
    };
  } catch (error) {
    await stop();
    throw error;
  }
}
