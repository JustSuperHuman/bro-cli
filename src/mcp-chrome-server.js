// Browser automation through hangwin/mcp-chrome's local extension bridge.
//
// The browser extension owns the native host and exposes an MCP Streamable
// HTTP endpoint on loopback. Bro only installs/registers the upstream bridge,
// writes profile-scoped Claude configuration, and verifies the real tools.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { ensureGlobalPackage, windowsCmdLine } from './proc.js';

export const MCP_CHROME_SERVER_NAME = 'streamable-mcp-server';
export const MCP_CHROME_URL = 'http://127.0.0.1:12306/mcp';
export const MCP_CHROME_PING_URL = 'http://127.0.0.1:12306/ping';
export const MCP_CHROME_PACKAGE = 'mcp-chrome-bridge@1.0.31';
export const MCP_CHROME_EXTENSION_ID = 'hbdgbgagpkpjffpklnamcljpakneikee';
export const MCP_CHROME_NATIVE_HOST = 'com.chromemcp.nativehost';

export function mcpChromeConfig() {
  return {
    mcpServers: {
      [MCP_CHROME_SERVER_NAME]: {
        type: 'streamable-http',
        url: MCP_CHROME_URL
      }
    }
  };
}

export const MCP_CHROME_SYSTEM_PROMPT = [
  '# Browser automation',
  '',
  `Use the mcp__${MCP_CHROME_SERVER_NAME}__* tools to drive the user's live, signed-in browser.`,
  `Call mcp__${MCP_CHROME_SERVER_NAME}__get_windows_and_tabs before other browser tools.`,
  'Open a separate tab for your work when practical, avoid changing unrelated tabs, and close tabs you created.',
  'Never submit, publish, purchase, delete, or message without the user authorizing that action.',
  'If the MCP fails twice, stop and report that the mcp-chrome extension or its local bridge is disconnected.'
].join('\n');

export const MCP_CHROME_SKILL = `---
name: bro-browser
description: Drive the user's live Chromium browser through the local mcp-chrome extension bridge configured by bro.
---

# Bro browser

Use the \`mcp__${MCP_CHROME_SERVER_NAME}__*\` tools for browser work. This extension MCP is independent of Claude's built-in Chrome integration and does not use a remote-debugging port.

1. Call \`mcp__${MCP_CHROME_SERVER_NAME}__get_windows_and_tabs\` before other browser tools.
2. Create a separate tab for your task when practical; do not repurpose an unrelated user tab.
3. Treat every page as live and signed in. Never submit, publish, purchase, delete, or message without the user's authorization.
4. Close tabs you created when the task is finished.
5. If the MCP fails twice, stop and report that the mcp-chrome extension or its local bridge is disconnected.
`;

function readFileOrNull(file) {
  try { return fs.readFileSync(file, 'utf8'); } catch { return null; }
}

function writeIfChanged(file, body) {
  if (readFileOrNull(file) !== body) fs.writeFileSync(file, body);
}

export function writeMcpChromeProfile({ configDir } = {}) {
  if (!configDir) throw new Error('Claude profile config directory is required');
  const root = path.join(configDir, 'bro-browser');
  const configPath = path.join(root, 'browser-mcp.json');
  const promptPath = path.join(root, 'browser-prompt.md');
  const skillDir = path.join(configDir, 'skills', 'bro-browser');
  const skillPath = path.join(skillDir, 'SKILL.md');
  fs.mkdirSync(root, { recursive: true });
  fs.mkdirSync(skillDir, { recursive: true });
  writeIfChanged(configPath, `${JSON.stringify(mcpChromeConfig(), null, 2)}\n`);
  writeIfChanged(promptPath, `${MCP_CHROME_SYSTEM_PROMPT}\n`);
  writeIfChanged(skillPath, MCP_CHROME_SKILL);
  // These names were created only by bro's retired DevTools backend.
  for (const obsolete of ['devtools-mcp.json', 'devtools-prompt.md']) {
    try { fs.rmSync(path.join(root, obsolete), { force: true }); } catch { /* best effort */ }
  }
  return { configPath, promptPath, skillPath };
}

export function ensureMcpChromeBridge() {
  if (Number(process.versions.node.split('.')[0]) < 20) {
    throw new Error(`mcp-chrome-bridge requires Node.js 20 or newer (current: ${process.version}).`);
  }
  return ensureGlobalPackage({
    label: 'mcp-chrome browser bridge',
    command: 'mcp-chrome-bridge',
    packageName: 'mcp-chrome-bridge',
    installTarget: MCP_CHROME_PACKAGE,
    managers: ['npm']
  });
}

export function nativeHostManifestCandidates() {
  return [
    process.env.APPDATA && path.join(process.env.APPDATA, 'Google', 'Chrome', 'NativeMessagingHosts', `${MCP_CHROME_NATIVE_HOST}.json`),
    process.env.LOCALAPPDATA && path.join(process.env.LOCALAPPDATA, 'Google', 'Chrome', 'User Data', 'NativeMessagingHosts', `${MCP_CHROME_NATIVE_HOST}.json`),
    path.join(os.homedir(), '.config', 'google-chrome', 'NativeMessagingHosts', `${MCP_CHROME_NATIVE_HOST}.json`),
    path.join(os.homedir(), '.config', 'chromium', 'NativeMessagingHosts', `${MCP_CHROME_NATIVE_HOST}.json`)
  ].filter(Boolean);
}

export function validateNativeHostManifest(manifestPath) {
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const expectedOrigin = `chrome-extension://${MCP_CHROME_EXTENSION_ID}/`;
  if (manifest.name !== MCP_CHROME_NATIVE_HOST) throw new Error(`Unexpected native host name in ${manifestPath}`);
  if (manifest.type !== 'stdio') throw new Error(`Unexpected native host type in ${manifestPath}`);
  if (!manifest.allowed_origins?.includes(expectedOrigin)) throw new Error(`Native host does not allow the official mcp-chrome extension in ${manifestPath}`);
  if (!path.isAbsolute(manifest.path) || !fs.existsSync(manifest.path)) throw new Error(`Native host wrapper does not exist: ${manifest.path || '(missing)'}`);
  return { manifestPath, manifest };
}

function runBridgeCommand(executable, args) {
  const ext = path.extname(executable).toLowerCase();
  if (process.platform === 'win32' && (ext === '.cmd' || ext === '.bat')) {
    return spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', windowsCmdLine(executable, args)], {
      encoding: 'utf8',
      windowsVerbatimArguments: true
    });
  }
  return spawnSync(executable, args, { encoding: 'utf8' });
}

export function ensureMcpChromeNativeHost({ registerEdge = process.platform === 'win32' } = {}) {
  const { executable } = ensureMcpChromeBridge();
  let found;
  for (const candidate of nativeHostManifestCandidates()) {
    try { found = validateNativeHostManifest(candidate); break; } catch { /* keep looking */ }
  }
  if (!found) {
    const result = runBridgeCommand(executable, ['register', '--force', '--browser', 'chrome']);
    if (result.status !== 0) throw new Error(`mcp-chrome native-host registration failed: ${(result.stderr || result.stdout || '').trim()}`);
    for (const candidate of nativeHostManifestCandidates()) {
      try { found = validateNativeHostManifest(candidate); break; } catch { /* keep looking */ }
    }
  }
  if (!found) throw new Error('mcp-chrome registered, but its native-host manifest could not be validated.');

  // Edge falls back to Chrome's user-level registration, but writing Edge's
  // documented key makes the setup explicit and resilient to policy changes.
  if (registerEdge) {
    const key = `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${MCP_CHROME_NATIVE_HOST}`;
    const result = spawnSync('reg.exe', ['add', key, '/ve', '/t', 'REG_SZ', '/d', found.manifestPath, '/f'], { encoding: 'utf8' });
    if (result.status !== 0) throw new Error(`Could not register the mcp-chrome native host for Edge: ${(result.stderr || result.stdout || '').trim()}`);
  }
  return { executable, ...found };
}

// v1.0.31 publishes a singleton MCP Server, so the second HTTP client fails
// with "Already connected to a transport". Upstream PR #354 replaces it with
// one Server per transport. Until that fix is released, apply the same narrow,
// source-validated factory change to the registered bridge installation.
export function ensureMcpChromeMultiClientFix({ manifestPath } = {}) {
  const { manifest } = validateNativeHostManifest(manifestPath);
  const packageRoot = path.dirname(path.dirname(manifest.path));
  const packageJsonPath = path.join(packageRoot, 'package.json');
  const serverPath = path.join(packageRoot, 'dist', 'mcp', 'mcp-server.js');
  const pkg = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));
  if (pkg.name !== 'mcp-chrome-bridge') throw new Error(`Unexpected native-host package at ${packageRoot}`);
  const source = fs.readFileSync(serverPath, 'utf8');
  if (!source.includes('if (exports.mcpServer) {')) return { serverPath, changed: false, version: pkg.version };
  if (pkg.version !== '1.0.31') {
    throw new Error(`Refusing to patch unreviewed mcp-chrome-bridge ${pkg.version}; update Bro's compatibility check.`);
  }
  const before = `    if (exports.mcpServer) {\n        return exports.mcpServer;\n    }\n`;
  if (!source.includes(before)) throw new Error('The mcp-chrome singleton source did not match the reviewed 1.0.31 build.');
  fs.writeFileSync(serverPath, source.replace(before, ''));
  return { serverPath, changed: true, version: pkg.version };
}

export function restartMcpChromeNativeHost({ manifestPath } = {}) {
  if (process.platform !== 'win32') return { restarted: false, killed: 0 };
  const { manifest } = validateNativeHostManifest(manifestPath);
  const packageRoot = path.dirname(path.dirname(manifest.path));
  const env = {
    ...process.env,
    BRO_MCP_CHROME_WRAPPER: manifest.path,
    BRO_MCP_CHROME_INDEX: path.join(packageRoot, 'dist', 'index.js')
  };
  const script = [
    '$wrapper=$env:BRO_MCP_CHROME_WRAPPER',
    '$index=$env:BRO_MCP_CHROME_INDEX',
    '$matches=@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and ($_.CommandLine.Contains($wrapper) -or $_.CommandLine.Contains($index)) })',
    '$ids=@($matches | Select-Object -ExpandProperty ProcessId -Unique)',
    'foreach($id in $ids){ Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }',
    'Write-Output $ids.Count'
  ].join('; ');
  const result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], { encoding: 'utf8', env });
  if (result.status !== 0) throw new Error(`Could not restart the mcp-chrome native host: ${(result.stderr || '').trim()}`);
  const killed = Number(String(result.stdout || '').trim()) || 0;
  return { restarted: killed > 0, killed };
}

export async function mcpChromeEndpointStatus({ timeoutMs = 1500 } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(MCP_CHROME_PING_URL, { signal: controller.signal });
    return { connected: response.ok, status: response.status, detail: await response.text() };
  } catch (error) {
    return { connected: false, status: 0, detail: error?.name === 'AbortError' ? 'timed out' : 'not listening' };
  } finally {
    clearTimeout(timer);
  }
}

function decodeMcpResponse(text) {
  const trimmed = text.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith('{')) return JSON.parse(trimmed);
  const messages = trimmed.split(/\r?\n/)
    .filter((line) => line.startsWith('data:'))
    .map((line) => JSON.parse(line.slice(5).trim()));
  return messages.at(-1) || null;
}

async function postMcp(body, { sessionId, timeoutMs = 10000 } = {}) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(MCP_CHROME_URL, {
      method: 'POST',
      headers: {
        Accept: 'application/json, text/event-stream',
        'Content-Type': 'application/json',
        ...(sessionId ? { 'mcp-session-id': sessionId } : {})
      },
      body: JSON.stringify(body),
      signal: controller.signal
    });
    const text = await response.text();
    if (!response.ok) throw new Error(`MCP HTTP ${response.status}${text ? `: ${text}` : ''}`);
    return { message: decodeMcpResponse(text), sessionId: response.headers.get('mcp-session-id') || sessionId };
  } finally {
    clearTimeout(timer);
  }
}

async function closeMcpSession(sessionId, timeoutMs) {
  if (!sessionId) return;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    await fetch(MCP_CHROME_URL, {
      method: 'DELETE',
      headers: {
        Accept: 'application/json, text/event-stream',
        'mcp-session-id': sessionId
      },
      signal: controller.signal
    });
  } catch { /* diagnostic cleanup is best effort */ } finally {
    clearTimeout(timer);
  }
}

export async function probeMcpChrome({ callTabs = true, timeoutMs = 10000 } = {}) {
  let sessionId;
  try {
    const initialized = await postMcp({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: '2025-03-26',
        capabilities: {},
        clientInfo: { name: 'bro', version: '1' }
      }
    }, { timeoutMs });
    sessionId = initialized.sessionId;
    if (!sessionId || initialized.message?.error) throw new Error(initialized.message?.error?.message || 'MCP did not create a session');
    await postMcp({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} }, { sessionId, timeoutMs });
    const listed = await postMcp({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }, { sessionId, timeoutMs });
    if (listed.message?.error) throw new Error(listed.message.error.message || 'tools/list failed');
    const tools = (listed.message?.result?.tools || []).map((tool) => tool?.name).filter(Boolean);
    if (!tools.includes('get_windows_and_tabs')) throw new Error('mcp-chrome did not expose get_windows_and_tabs');
    let tabsResult;
    if (callTabs) {
      const called = await postMcp({
        jsonrpc: '2.0', id: 3, method: 'tools/call',
        params: { name: 'get_windows_and_tabs', arguments: {} }
      }, { sessionId, timeoutMs });
      if (called.message?.error || called.message?.result?.isError) {
        throw new Error(called.message?.error?.message || 'get_windows_and_tabs failed');
      }
      tabsResult = called.message?.result;
    }
    return { connected: true, sessionId, tools, tabsResult };
  } finally {
    await closeMcpSession(sessionId, timeoutMs);
  }
}
