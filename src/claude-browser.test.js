import { expect, test } from 'bun:test';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  CHROME_MCP_SYSTEM_PROMPT,
  accountUuidOf,
  adoptLegacyEdgeProfile,
  bridgePipeLive,
  browserFromUserAgent,
  browserBackend,
  browserOwnerConfigDir,
  browsersWithClaudeExtension,
  browsersWithMcpChromeExtension,
  claudeBrowserMode,
  EXTENSION_RECONNECT_URL,
  ensureBrowserConnected,
  ensurePatchedExtension,
  findBrowserExecutable,
  listClaudeLogins,
  nudgeExtensionReconnect,
  openClaudeBrowser,
  patchClaudeExtensionWorker,
  pairedDevice,
  pinPairedDevice,
  preferredBrowser,
  prepareClaudeBrowser,
  probeConnectedBrowsers,
  resolveBrowser,
  scrubBridgeEnv,
  setBrowserOwner,
  setClaudeBrowserEnabled,
  setPairedDevice,
  setPreferredBrowser,
  sharedBridgeId,
  sharedUserDataDir,
  stockBridgePipeLive,
  usesThirdPartyAuth,
  withBrowserBridgeEnv,
  writeChromeMcpConfig
} from './claude-browser.js';
import { CHROME_MCP_FLAG, CHROME_MCP_SERVER_NAME, browserFromNewTabUrl, chromeMcpConfig } from './chrome-mcp.js';
import { MCP_CHROME_SERVER_NAME, MCP_CHROME_URL } from './mcp-chrome-server.js';

const DESKTOP = '{name:"com.anthropic.claude_browser_extension",label:"Desktop"}';
const CODE = '{name:"com.anthropic.claude_code_browser_extension",label:"Claude Code"}';

// The test runner itself may carry a bro session's bridge env (BUN_OPTIONS
// preload + BRO_CLAUDE_BROWSER_ID), which would silently rewrite any pipe this
// process touches. Subprocess tests therefore always start from this env.
const cleanEnv = { ...process.env };
delete cleanEnv.BRO_CLAUDE_BROWSER_ID;
delete cleanEnv.BUN_OPTIONS;
delete cleanEnv.BRO_CLAUDE_BROWSER_TRACE_FILE;

function tempRoot(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function removeTemp(dir) {
  if (path.dirname(path.resolve(dir)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
  fs.rmSync(dir, { recursive: true, force: true });
}

test('the shared bridge id is stable per browser root', () => {
  const root = path.join(os.tmpdir(), 'bro-browser-a');
  const id = sharedBridgeId(root);
  expect(id).toHaveLength(12);
  expect(sharedBridgeId(root)).toBe(id);
  expect(sharedBridgeId(path.join(os.tmpdir(), 'bro-browser-b'))).not.toBe(id);
});

test('the patched extension prefers Claude Code without modifying other worker code', () => {
  const input = `before;const hosts=[${DESKTOP},${CODE}];after`;
  expect(patchClaudeExtensionWorker(input)).toBe(`before;const hosts=[${CODE},${DESKTOP}];after`);
  expect(() => patchClaudeExtensionWorker('no native hosts here')).toThrow(/changed its native-host bootstrap/);
});

test('extension duplication patches only the copy and reuses its fingerprinted result', () => {
  const temp = tempRoot('bro-browser-extension-');
  const source = path.join(temp, 'source');
  const root = path.join(temp, 'bro');
  const workerRelative = path.join('assets', 'service-worker.js');
  const workerText = `const hosts=[${DESKTOP},${CODE}];`;

  try {
    fs.mkdirSync(path.join(source, 'assets'), { recursive: true });
    fs.writeFileSync(path.join(source, 'manifest.json'), JSON.stringify({ version: '1.2.3' }));
    fs.writeFileSync(path.join(source, 'service-worker-loader.js'), "import './assets/service-worker.js';\n");
    fs.writeFileSync(path.join(source, workerRelative), workerText);

    const first = ensurePatchedExtension({ root, installed: { dir: source, version: '1.2.3' } });
    const second = ensurePatchedExtension({ root, installed: { dir: source, version: '1.2.3' } });
    expect(second.dir).toBe(first.dir);
    expect(fs.readFileSync(path.join(source, workerRelative), 'utf8')).toBe(workerText);
    expect(fs.readFileSync(path.join(first.dir, workerRelative), 'utf8')).toContain(`${CODE},${DESKTOP}`);
  } finally {
    removeTemp(temp);
  }
});

test('browser bridge environment preserves existing Bun options and never doubles the preload', () => {
  const env = withBrowserBridgeEnv(
    { BUN_OPTIONS: '--smol', KEEP: 'yes' },
    { id: 'abc123', preloadPath: 'C:\\Users\\Test User\\bridge.cjs' }
  );
  expect(env.KEEP).toBe('yes');
  expect(env.BRO_CLAUDE_BROWSER_ID).toBe('abc123');
  expect(env.BUN_OPTIONS).toBe('--smol --preload=C:/Users/Test User/bridge.cjs');

  const layered = withBrowserBridgeEnv(env, { id: 'def456', preloadPath: 'C:\\Users\\Test User\\bridge.cjs' });
  expect(layered.BRO_CLAUDE_BROWSER_ID).toBe('def456');
  expect(layered.BUN_OPTIONS).toBe('--smol --preload=C:/Users/Test User/bridge.cjs');
});

test('bridge liveness matches only this root\'s pipe namespace', () => {
  const pipeNames = [
    'claude-mcp-browser-bridge-james',
    'claude-mcp-browser-bridge-bro-aaaabbbbcccc-james'
  ];
  expect(bridgePipeLive('aaaabbbbcccc', { pipeNames })).toBe(true);
  expect(bridgePipeLive('aaaabbbbcc', { pipeNames })).toBe(false);
  expect(bridgePipeLive('ddddeeeeffff', { pipeNames })).toBe(false);
});

test('the most recently used legacy per-login browser becomes the shared one', () => {
  const root = tempRoot('bro-browser-adopt-');
  try {
    const legacy = path.join(root, 'edge-profiles');
    for (const [name, age] of [['old-11111111', 60], ['recent-22222222', 5]]) {
      fs.mkdirSync(path.join(legacy, name), { recursive: true });
      fs.writeFileSync(path.join(legacy, name, 'Local State'), '{}');
      const when = new Date(Date.now() - age * 60000);
      fs.utimesSync(path.join(legacy, name, 'Local State'), when, when);
    }
    const adopted = adoptLegacyEdgeProfile(root);
    expect(adopted).toEqual({ name: 'recent-22222222' });
    expect(fs.existsSync(path.join(sharedUserDataDir(root), 'Local State'))).toBe(true);
    expect(fs.existsSync(path.join(legacy, 'recent-22222222'))).toBe(false);
    expect(fs.existsSync(path.join(legacy, 'old-11111111'))).toBe(true);
    expect(adoptLegacyEdgeProfile(root)).toBe(null);
  } finally {
    removeTemp(root);
  }
});

test.skipIf(process.platform !== 'win32')('a still-running legacy browser blocks adoption loudly instead of forfeiting it', () => {
  const root = tempRoot('bro-browser-locked-');
  try {
    const dir = path.join(root, 'edge-profiles', 'busy-33333333');
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, 'Local State'), '{}');
    const handle = fs.openSync(path.join(dir, 'Local State'), 'r');
    try {
      expect(() => adoptLegacyEdgeProfile(root)).toThrow(/still open/);
    } finally {
      fs.closeSync(handle);
    }
    expect(adoptLegacyEdgeProfile(root)).toEqual({ name: 'busy-33333333' });
  } finally {
    removeTemp(root);
  }
});

test('the shared browser opens with the shared profile, extension, and bridge namespace', async () => {
  const root = tempRoot('bro-browser-open-');
  try {
    const infrastructure = {
      root,
      edgePath: path.join(root, 'msedge.exe'),
      preloadPath: path.join(root, 'claude-browser-preload.cjs'),
      extension: { dir: path.join(root, 'extensions', '1.0-abc'), version: '1.0' },
      nativeHost: {}
    };
    const calls = [];
    const spawnBrowser = (command, args, options) => {
      calls.push({ command, args, options });
      return { on() {}, unref() {} };
    };

    const first = await openClaudeBrowser({ root, infrastructure, spawnBrowser, settleMs: 0 });
    expect(first.id).toBe(sharedBridgeId(root));
    expect(first.firstRun).toBe(true);
    expect(calls[0].args).toContain(`--user-data-dir=${sharedUserDataDir(root)}`);
    expect(calls[0].args).toContain(`--load-extension=${infrastructure.extension.dir}`);
    expect(calls[0].args).toContain('https://claude.ai/');
    expect(calls[0].options.env.BRO_CLAUDE_BROWSER_ID).toBe(first.id);
    expect(calls[0].options.env.BUN_OPTIONS).toContain('--preload=');

    fs.writeFileSync(path.join(sharedUserDataDir(root), 'Local State'), '{}');
    const second = await openClaudeBrowser({ root, infrastructure, spawnBrowser, settleMs: 0 });
    expect(second.firstRun).toBe(false);
    expect(calls[1].args).not.toContain('https://claude.ai/');
  } finally {
    removeTemp(root);
  }
});

test('session launches skip the browser entirely while it is disabled', async () => {
  const root = tempRoot('bro-browser-disabled-');
  try {
    expect(await prepareClaudeBrowser({ root })).toBe(null);
  } finally {
    removeTemp(root);
  }
});

// The reason bro configures the browser server itself: Claude Code only wires
// its own when the session authenticates as a claude.ai account.
test('third-party auth is what decides between --chrome and the browser server', () => {
  expect(usesThirdPartyAuth({})).toBe(false);
  expect(usesThirdPartyAuth({ ANTHROPIC_BASE_URL: 'https://api.z.ai/api/anthropic' })).toBe(false);
  expect(usesThirdPartyAuth({ ANTHROPIC_AUTH_TOKEN: 'zai-key' })).toBe(true);
  expect(usesThirdPartyAuth({ ANTHROPIC_API_KEY: 'sk-ant-x' })).toBe(true);
});

test('a claude.ai login keeps Claude Code’s own --chrome wiring', async () => {
  const root = tempRoot('bro-browser-native-');
  try {
    setClaudeBrowserEnabled(true, root);
    const prepared = await prepareClaudeBrowser({
      verifyConnection: false,
      root,
      claudePath: 'claude.exe',
      baseEnv: {},
      spawnBrowser: () => ({ on() {}, unref() {} }),
      settleMs: 0,
      pipeNames: ['claude-mcp-browser-bridge-tester']
    });
    expect(prepared.bridged).toBe(false);
    expect(prepared.args).toEqual(['--chrome']);
  } finally {
    removeTemp(root);
  }
});

// A scripted run still gets the browser tools, and still joins a browser that
// is already up — it just must not raise a window on anyone's screen.
test('a headless run joins a running browser but never starts one', async () => {
  const root = tempRoot('bro-browser-autostart-');
  try {
    setClaudeBrowserEnabled(true, root);
    const started = [];
    const prepare = (options) => prepareClaudeBrowser({
      verifyConnection: false,
      root,
      claudePath: 'claude.exe',
      baseEnv: {},
      spawnBrowser: (...call) => { started.push(call); return { on() {}, unref() {} }; },
      settleMs: 0,
      ...options
    });

    // Nothing is serving the bridge, so an interactive launch opens a browser.
    const interactive = await prepare({ pipeNames: [] });
    expect(interactive.args).toEqual(['--chrome']);
    expect(started).toHaveLength(1);

    // Same situation, headless: the tools are wired, the window is not opened.
    const headless = await prepare({ pipeNames: [], autoStart: false });
    expect(headless.args).toEqual(['--chrome']);
    expect(started).toHaveLength(1);

    // And with a browser already up, headless attaches like anything else.
    const joined = await prepare({ pipeNames: ['claude-mcp-browser-bridge-tester'], autoStart: false });
    expect(joined.args).toEqual(['--chrome']);
    expect(started).toHaveLength(1);
  } finally {
    removeTemp(root);
  }
});

test('a provider-backed session is handed the browser server instead', async () => {
  const root = tempRoot('bro-browser-bridged-');
  try {
    setClaudeBrowserEnabled(true, root);
    const prepared = await prepareClaudeBrowser({
      verifyConnection: false,
      root,
      claudePath: 'claude.exe',
      baseEnv: { ANTHROPIC_AUTH_TOKEN: 'zai-key' },
      skipPermissions: true,
      spawnBrowser: () => ({ on() {}, unref() {} }),
      settleMs: 0,
      pipeNames: ['claude-mcp-browser-bridge-tester']
    });
    expect(prepared.bridged).toBe(true);
    expect(prepared.args[0]).toBe('--mcp-config');
    expect(prepared.args[2]).toBe('--append-system-prompt-file');
    expect(fs.readFileSync(prepared.args[3], 'utf8').trim()).toBe(CHROME_MCP_SYSTEM_PROMPT);
    // Never Claude Code's flag as well: it would be accepted and then ignored.
    expect(prepared.args).not.toContain('--chrome');
    // No argument may contain a newline: the proxy route spawns through a
    // .cmd shim whose command line is one string.
    for (const arg of prepared.args) expect(arg).not.toContain('\n');

    const written = JSON.parse(fs.readFileSync(prepared.args[1], 'utf8'));
    expect(written.mcpServers[CHROME_MCP_SERVER_NAME]).toMatchObject({
      type: 'stdio',
      command: 'claude.exe',
      args: [CHROME_MCP_FLAG],
      env: { CLAUDE_CHROME_PERMISSION_MODE: 'skip_all_permission_checks' }
    });
  } finally {
    removeTemp(root);
  }
});

test('the mcp-chrome backend always uses profile HTTP MCP and disables built-in Chrome', async () => {
  const root = tempRoot('bro-browser-mcp-chrome-');
  const profile = path.join(root, 'profile');
  try {
    fs.mkdirSync(profile);
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({
      enabled: true,
      mode: 'main',
      backend: 'mcp-chrome',
      browser: 'Microsoft Edge'
    }));
    expect(browserBackend(root)).toBe('mcp-chrome');
    const prepared = await prepareClaudeBrowser({
      root,
      baseEnv: { CLAUDE_CONFIG_DIR: profile },
      claudePath: 'claude.exe',
      autoStart: false
    });
    expect(prepared.backend).toBe('mcp-chrome');
    expect(prepared.args).toContain('--no-chrome');
    expect(prepared.args).not.toContain('--chrome');
    expect(prepared.env.CLAUDE_CODE_ENABLE_CFC).toBe('false');
    expect(prepared.configPath.startsWith(profile)).toBe(true);
    expect(prepared.skillPath).toBe(path.join(profile, 'skills', 'bro-browser', 'SKILL.md'));
    const config = JSON.parse(fs.readFileSync(prepared.configPath, 'utf8'));
    expect(config.mcpServers[MCP_CHROME_SERVER_NAME]).toEqual({
      type: 'streamable-http',
      url: MCP_CHROME_URL
    });
    expect(fs.readFileSync(prepared.skillPath, 'utf8'))
      .toContain(`mcp__${MCP_CHROME_SERVER_NAME}__get_windows_and_tabs`);
  } finally {
    removeTemp(root);
  }
});

test('legacy DevTools state migrates to mcp-chrome without launching a debugging browser', async () => {
  const root = tempRoot('bro-browser-legacy-devtools-');
  const profile = path.join(root, 'profile');
  try {
    fs.mkdirSync(profile);
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, backend: 'devtools' }));
    const prepared = await prepareClaudeBrowser({ root, baseEnv: { CLAUDE_CONFIG_DIR: profile }, autoStart: false });
    expect(browserBackend(root)).toBe('mcp-chrome');
    expect(prepared.backend).toBe('mcp-chrome');
    expect(prepared.args.join(' ')).not.toContain('9222');
    expect(prepared.args.join(' ')).not.toContain('9223');
  } finally {
    removeTemp(root);
  }
});

// "claude-in-chrome" is reserved by Claude Code and silently dropped from
// --mcp-config, so bro's copy must answer to something else.
test('the browser server avoids Claude Code’s reserved name', () => {
  expect(CHROME_MCP_SERVER_NAME).not.toBe('claude-in-chrome');
  expect(Object.keys(chromeMcpConfig({ claudePath: 'claude.exe' }).mcpServers)).toEqual([CHROME_MCP_SERVER_NAME]);
});

test('permission mode is only forced onto the browser when the session already bypasses it', () => {
  const root = tempRoot('bro-browser-perms-');
  try {
    const asking = JSON.parse(fs.readFileSync(writeChromeMcpConfig({ root, claudePath: 'claude.exe' }), 'utf8'));
    expect(asking.mcpServers[CHROME_MCP_SERVER_NAME].env.CLAUDE_CHROME_PERMISSION_MODE).toBeUndefined();
    // Different files, so two concurrent sessions cannot rewrite each other's.
    expect(writeChromeMcpConfig({ root, claudePath: 'claude.exe' }))
      .not.toBe(writeChromeMcpConfig({ root, claudePath: 'claude.exe', skipPermissions: true }));
  } finally {
    removeTemp(root);
  }
});

// The bridge only answers the claude.ai account the extension is signed into,
// so the browser server always runs as that login — the "owner" — whichever
// login the session itself uses.
test('the browser server always identifies as the owner login', () => {
  const root = tempRoot('bro-browser-owner-env-');
  try {
    const server = JSON.parse(fs.readFileSync(
      writeChromeMcpConfig({ root, claudePath: 'claude.exe', ownerConfigDir: 'C:\\owner' }), 'utf8'
    )).mcpServers[CHROME_MCP_SERVER_NAME];
    expect(server.env.CLAUDE_CONFIG_DIR).toBe('C:\\owner');
    // Neutralizes the "false" a foreign-login session carries to keep its own
    // built-in wiring quiet — the server must not inherit it.
    expect(server.env.CLAUDE_CODE_ENABLE_CFC).toBe('true');
  } finally {
    removeTemp(root);
  }
});

test('the browser owner defaults to this machine’s login and follows the state', () => {
  const root = tempRoot('bro-browser-owner-state-');
  try {
    const localDir = path.join(root, 'local');
    const accountsDir = path.join(root, 'accounts');
    expect(browserOwnerConfigDir(root, { localDir, accountsDir })).toBe(localDir);
    setBrowserOwner('claude-1', root);
    expect(browserOwnerConfigDir(root, { localDir, accountsDir })).toBe(path.join(accountsDir, 'claude-1'));
    setBrowserOwner('local', root);
    expect(browserOwnerConfigDir(root, { localDir, accountsDir })).toBe(localDir);
  } finally {
    removeTemp(root);
  }
});

// A pool account is a real claude.ai login, but not the one the extension
// answers — Claude Code's own wiring would identify as it and be told no
// browsers are connected. Those sessions get the browser server instead.
test('a login on a different account than the owner is handed the browser server', async () => {
  const root = tempRoot('bro-browser-foreign-');
  try {
    setClaudeBrowserEnabled(true, root);
    const ownerDir = path.join(root, 'owner');
    const poolDir = path.join(root, 'pool-account');
    fs.mkdirSync(ownerDir, { recursive: true });
    fs.mkdirSync(poolDir, { recursive: true });
    fs.writeFileSync(path.join(ownerDir, '.claude.json'), JSON.stringify({ oauthAccount: { accountUuid: 'AAAA-1111' } }));
    fs.writeFileSync(path.join(poolDir, '.claude.json'), JSON.stringify({ oauthAccount: { accountUuid: 'BBBB-2222' } }));
    expect(accountUuidOf(ownerDir)).toBe('aaaa-1111');

    const prepare = (baseEnv, ownerConfigDir) => prepareClaudeBrowser({
      verifyConnection: false,
      root,
      claudePath: 'claude.exe',
      baseEnv,
      ownerConfigDir,
      spawnBrowser: () => ({ on() {}, unref() {} }),
      settleMs: 0,
      pipeNames: ['claude-mcp-browser-bridge-tester']
    });

    const foreign = await prepare({ CLAUDE_CONFIG_DIR: poolDir }, ownerDir);
    expect(foreign.bridged).toBe(true);
    expect(foreign.args[0]).toBe('--mcp-config');
    // The session's own account-bound wiring is switched off so the model
    // never sees browser tools that answer with an empty browser list…
    expect(foreign.env.CLAUDE_CODE_ENABLE_CFC).toBe('false');
    // …and the server it does get identifies as the owner.
    const server = JSON.parse(fs.readFileSync(foreign.args[1], 'utf8')).mcpServers[CHROME_MCP_SERVER_NAME];
    expect(server.env.CLAUDE_CONFIG_DIR).toBe(ownerDir);

    // The owner login itself keeps Claude Code's native wiring.
    const native = await prepare({ CLAUDE_CONFIG_DIR: ownerDir }, ownerDir);
    expect(native.bridged).toBe(false);
    expect(native.args).toEqual(['--chrome']);
    expect(native.env.CLAUDE_CODE_ENABLE_CFC).toBeUndefined();

    // As does any other login signed in to the same account: the extension
    // answers the account, not the directory.
    const twinDir = path.join(root, 'twin');
    fs.mkdirSync(twinDir, { recursive: true });
    fs.writeFileSync(path.join(twinDir, '.claude.json'), JSON.stringify({ oauthAccount: { accountUuid: 'aaaa-1111' } }));
    const twin = await prepare({ CLAUDE_CONFIG_DIR: twinDir }, ownerDir);
    expect(twin.args).toEqual(['--chrome']);
  } finally {
    removeTemp(root);
  }
});

test('a browser choice persists and drops the device it used to resolve to', () => {
  const root = tempRoot('bro-browser-choice-');
  try {
    expect(preferredBrowser(root)).toBe('');
    setPreferredBrowser('Microsoft Edge', root);
    setPairedDevice({ deviceId: 'device-edge', browser: 'Microsoft Edge' }, root);
    expect(preferredBrowser(root)).toBe('Microsoft Edge');
    expect(pairedDevice(root)).toEqual({ deviceId: 'device-edge', browser: 'Microsoft Edge' });

    setPreferredBrowser('Google Chrome', root);
    expect(pairedDevice(root)).toBe(null);
    // Re-choosing the same browser leaves a resolved device alone.
    setPairedDevice({ deviceId: 'device-chrome', browser: 'Google Chrome' }, root);
    setPreferredBrowser('Google Chrome', root);
    expect(pairedDevice(root)?.deviceId).toBe('device-chrome');
  } finally {
    removeTemp(root);
  }
});

test('browsers resolve from an id, a display name or an unambiguous prefix', () => {
  expect(resolveBrowser('edge').name).toBe('Microsoft Edge');
  expect(resolveBrowser('Microsoft Edge').id).toBe('edge');
  expect(resolveBrowser('chro').id).toBe('chrome');
  expect(resolveBrowser('firefox')).toBe(null);
  expect(resolveBrowser('')).toBe(null);
});

test('new tab pages name the browser without opening a page', () => {
  expect(browserFromNewTabUrl('edge://newtab/')).toBe('Microsoft Edge');
  expect(browserFromNewTabUrl('chrome://newtab/')).toBe('Google Chrome');
  expect(browserFromNewTabUrl('brave://newtab/')).toBe('Brave');
  expect(browserFromNewTabUrl('https://example.com')).toBe(null);
  expect(browserFromNewTabUrl('')).toBe(null);
});

test('browser executables are looked up per browser, honouring BRO_EDGE_PATH', () => {
  const temp = tempRoot('bro-browser-exe-');
  try {
    const edge = path.join(temp, 'msedge.exe');
    const chrome = path.join(temp, 'Google', 'Chrome', 'Application', 'chrome.exe');
    fs.writeFileSync(edge, '');
    fs.mkdirSync(path.dirname(chrome), { recursive: true });
    fs.writeFileSync(chrome, '');
    expect(findBrowserExecutable('edge', { BRO_EDGE_PATH: edge })).toBe(edge);
    expect(findBrowserExecutable('chrome', { PROGRAMFILES: temp })).toBe(chrome);
    expect(findBrowserExecutable('brave', { PROGRAMFILES: temp })).toBe(null);
  } finally {
    removeTemp(temp);
  }
});

// The pin has to land in .claude.json because that is the file the bridge
// consults when more than one browser is connected.
test('pinning a device writes it where the bridge reads it, and clearing removes it', () => {
  const temp = tempRoot('bro-browser-pin-');
  try {
    const file = path.join(temp, '.claude.json');
    fs.writeFileSync(file, JSON.stringify({ oauthAccount: { emailAddress: 'a@example.com' } }));
    expect(pinPairedDevice({ configDir: temp, deviceId: 'device-edge', browser: 'Microsoft Edge' })).toBe(true);
    expect(JSON.parse(fs.readFileSync(file, 'utf8'))).toMatchObject({
      oauthAccount: { emailAddress: 'a@example.com' },
      chromeExtension: { pairedDeviceId: 'device-edge', pairedDeviceName: 'Microsoft Edge' }
    });

    pinPairedDevice({ configDir: temp, deviceId: '' });
    expect(JSON.parse(fs.readFileSync(file, 'utf8')).chromeExtension).toBeUndefined();
    // A login Claude Code has never written to has nothing to pin.
    expect(pinPairedDevice({ configDir: path.join(temp, 'missing'), deviceId: 'x' })).toBe(false);
  } finally {
    removeTemp(temp);
  }
});

test('logins list covers the local login and every pool account with auth state', () => {
  const temp = tempRoot('bro-browser-logins-');
  try {
    const accounts = path.join(temp, 'accounts');
    const local = path.join(temp, 'local');
    fs.mkdirSync(path.join(accounts, 'work'), { recursive: true });
    fs.mkdirSync(local, { recursive: true });
    fs.writeFileSync(path.join(accounts, 'work', '.credentials.json'), JSON.stringify({ claudeAiOauth: { accessToken: 'x' } }));
    fs.writeFileSync(path.join(accounts, 'work', '.claude.json'), JSON.stringify({ oauthAccount: { emailAddress: 'work@example.com' } }));
    const logins = listClaudeLogins({ accountsDir: accounts, defaultConfigDir: local });
    expect(logins.map((login) => login.name)).toEqual(['local', 'work']);
    expect(logins[0].authenticated).toBe(false);
    expect(logins[1]).toMatchObject({ authenticated: true, email: 'work@example.com', local: false });
  } finally {
    removeTemp(temp);
  }
});

test('user agents map to the browser that actually served the session', () => {
  const chromium = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko)';
  expect(browserFromUserAgent(`${chromium} Chrome/151.0.0.0 Safari/537.36 Edg/151.0.0.0`)).toBe('Microsoft Edge');
  expect(browserFromUserAgent(`${chromium} Chrome/151.0.0.0 Safari/537.36`)).toContain('Chrome');
  expect(browserFromUserAgent(`${chromium} Chrome/151.0.0.0 Safari/537.36 OPR/117.0.0.0`)).toBe('Opera');
  expect(browserFromUserAgent('Mozilla/5.0 Gecko/20100101 Firefox/143.0')).toBe(null);
});

test('every browser carrying the Claude extension is reported', () => {
  const local = tempRoot('bro-browser-scan-');
  try {
    const extension = 'fcoeoabgfenejglbffodgkkbkcdhcgfn';
    fs.mkdirSync(path.join(local, 'Microsoft', 'Edge', 'User Data', 'Default', 'Extensions', extension), { recursive: true });
    fs.mkdirSync(path.join(local, 'Google', 'Chrome', 'User Data', 'Profile 1', 'Extensions', extension), { recursive: true });
    fs.mkdirSync(path.join(local, 'BraveSoftware', 'Brave-Browser', 'User Data', 'Default'), { recursive: true });
    expect(browsersWithClaudeExtension({ LOCALAPPDATA: local })).toEqual(['Microsoft Edge', 'Google Chrome']);
  } finally {
    removeTemp(local);
  }
});

test('an unpacked mcp-chrome extension is detected from Chromium secure preferences', () => {
  const local = tempRoot('bro-browser-mcp-extension-scan-');
  try {
    const profile = path.join(local, 'Microsoft', 'Edge', 'User Data', 'Default');
    fs.mkdirSync(profile, { recursive: true });
    fs.writeFileSync(path.join(profile, 'Secure Preferences'), JSON.stringify({
      extensions: {
        settings: {
          hbdgbgagpkpjffpklnamcljpakneikee: {
            state: 1,
            path: 'C:\\mcp-chrome'
          }
        }
      }
    }));
    expect(browsersWithMcpChromeExtension({ LOCALAPPDATA: local })).toEqual(['Microsoft Edge']);
  } finally {
    removeTemp(local);
  }
});

test('main mode is the default and dedicated mode is explicit', () => {
  const root = tempRoot('bro-browser-mode-');
  try {
    expect(claudeBrowserMode(root)).toBe('main');
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, mode: 'dedicated' }));
    expect(claudeBrowserMode(root)).toBe('dedicated');
  } finally {
    removeTemp(root);
  }
});

test('the stock bridge pipe is recognized apart from bro namespaces', () => {
  expect(stockBridgePipeLive({ pipeNames: ['claude-mcp-browser-bridge-james'] })).toBe(true);
  expect(stockBridgePipeLive({ pipeNames: ['claude-mcp-browser-bridge-bro-aaaabbbbcccc-james'] })).toBe(false);
  expect(stockBridgePipeLive({ pipeNames: ['other-pipe'] })).toBe(false);
});

test('scrubbing removes only bro\'s bridge namespace from a session environment', () => {
  const env = scrubBridgeEnv({
    BRO_CLAUDE_BROWSER_ID: '46d33d2adfc5',
    BUN_OPTIONS: '--smol --preload=C:/Users/x/.bro/claude-browser/claude-browser-preload.cjs',
    KEEP: 'yes'
  });
  expect(env.BRO_CLAUDE_BROWSER_ID).toBe('');
  expect(env.BUN_OPTIONS).toBe('--smol');
  expect(env.KEEP).toBe('yes');
});

test('main mode attaches sessions to the stock pipe without starting a browser when it is live', async () => {
  const root = tempRoot('bro-browser-main-');
  try {
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, mode: 'main' }));
    let spawned = 0;
    const prepared = await prepareClaudeBrowser({
      verifyConnection: false,
      root,
      baseEnv: {
        BRO_CLAUDE_BROWSER_ID: 'stale',
        BUN_OPTIONS: '--preload=C:/x/claude-browser-preload.cjs'
      },
      pipeNames: ['claude-mcp-browser-bridge-james'],
      spawnBrowser: () => { spawned += 1; return { on() {}, unref() {} }; }
    });
    expect(prepared.mode).toBe('main');
    expect(prepared.env.BRO_CLAUDE_BROWSER_ID).toBe('');
    expect(prepared.env.BUN_OPTIONS).toBe('');
    expect(spawned).toBe(0);
  } finally {
    removeTemp(root);
  }
});

test.skipIf(process.platform !== 'win32')('main mode starts the main browser when no bridge is up', async () => {
  const root = tempRoot('bro-browser-main-start-');
  try {
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, mode: 'main' }));
    const calls = [];
    const prepared = await prepareClaudeBrowser({
      verifyConnection: false,
      root,
      baseEnv: {},
      pipeNames: ['unrelated-pipe'],
      settleMs: 0,
      spawnBrowser: (command, args) => { calls.push({ command, args }); return { on() {}, unref() {} }; }
    });
    expect(prepared.mode).toBe('main');
    expect(calls).toHaveLength(1);
    expect(calls[0].args).toEqual([]);
  } finally {
    removeTemp(root);
  }
});

test.skipIf(process.platform !== 'win32')('the Bun preload sends bridge clients to the shared named pipe', async () => {
  const id = `test-${process.pid}-${Date.now()}`;
  const username = os.userInfo().username;
  const normal = `\\\\.\\pipe\\claude-mcp-browser-bridge-${username}`;
  const shared = `\\\\.\\pipe\\claude-mcp-browser-bridge-bro-${id}-${username}`;
  const preload = fileURLToPath(new URL('./claude-browser-preload.cjs', import.meta.url));

  const collect = (child) => {
    const output = { stdout: '', stderr: '' };
    child.stdout?.on('data', (chunk) => { output.stdout += chunk; });
    child.stderr?.on('data', (chunk) => { output.stderr += chunk; });
    return output;
  };

  // The unpatched server child owns the pipe under its final shared name; the
  // patched client child dials the stock name and must be rewritten to it.
  const serverCode = [
    "const net=require('net')",
    "const server=net.createServer(socket=>{socket.end();server.close(()=>console.log('CONNECTED'))})",
    `server.listen(${JSON.stringify(shared)},()=>console.log('READY'))`,
    "setTimeout(()=>{console.error('TIMEOUT');process.exit(1)},8000).unref()"
  ].join(';');
  const server = spawn(process.execPath, ['-e', serverCode], { env: cleanEnv, stdio: ['ignore', 'pipe', 'pipe'] });
  const serverOutput = collect(server);
  const serverExit = new Promise((resolve) => server.on('exit', resolve));

  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`pipe server never became ready: ${serverOutput.stderr}`)), 8000);
      server.stdout.on('data', () => {
        if (serverOutput.stdout.includes('READY')) { clearTimeout(timer); resolve(); }
      });
      server.on('exit', () => reject(new Error(`pipe server exited early: ${serverOutput.stderr}`)));
    });

    const clientCode = [
      "const net=require('net')",
      `const socket=net.connect(${JSON.stringify(normal)},()=>socket.end())`,
      "socket.on('error',error=>{console.error(error.message);process.exitCode=1})"
    ].join(';');
    const client = spawn(process.execPath, ['-e', clientCode], {
      env: {
        ...cleanEnv,
        BRO_CLAUDE_BROWSER_ID: id,
        BUN_OPTIONS: `--preload=${preload.replace(/\\/g, '/')}`
      },
      stdio: ['ignore', 'ignore', 'pipe']
    });
    const clientOutput = collect(client);
    const clientExit = await new Promise((resolve) => client.on('exit', resolve));
    expect(clientOutput.stderr).toBe('');
    expect(clientExit).toBe(0);
    expect(await serverExit).toBe(0);
    expect(serverOutput.stdout).toContain('CONNECTED');
  } finally {
    try { server.kill(); } catch {}
  }
});

// --- staying on the bridge ------------------------------------------------
//
// A live pipe proves nothing here: Claude Desktop's native host answers the
// same host name and holds the same pipe, while the browser tools read a
// per-account room on Anthropic's bridge instead. Every readiness answer
// therefore comes from asking the bridge — and an empty answer is recoverable
// before the session that would have tripped over it even starts.

// A probe that answers a different thing each call, then repeats the last one.
const answering = (...rounds) => async () => (rounds.length > 1 ? rounds.shift() : rounds[0]);

test('the bridge itself decides whether a browser is reachable', async () => {
  const root = tempRoot('bro-browser-probe-');
  try {
    setClaudeBrowserEnabled(true, root);
    const seen = [];
    const probed = await probeConnectedBrowsers({
      claudePath: 'claude.exe',
      root,
      env: { ANTHROPIC_AUTH_TOKEN: 'provider-key', BRO_CLAUDE_BROWSER_ID: 'stale' },
      ownerConfigDir: 'C:/owner',
      openBridge: async (options) => {
        seen.push(options);
        return { callToolJson: async () => [{ deviceId: 'device-a' }], close() {} };
      }
    });
    expect(probed.devices).toEqual([{ deviceId: 'device-a' }]);
    expect(probed.error).toBe(null);
    // The probe is only worth anything if it is the same server the session
    // gets: owner login, bro's pipe namespace scrubbed, CFC switched back on.
    expect(seen[0].env.CLAUDE_CONFIG_DIR).toBe('C:/owner');
    expect(seen[0].env.CLAUDE_CODE_ENABLE_CFC).toBe('true');
    expect(seen[0].env.BRO_CLAUDE_BROWSER_ID).toBe('');

    const unreachable = await probeConnectedBrowsers({
      claudePath: 'claude.exe',
      root,
      openBridge: async () => { throw new Error('the browser bridge exited (code 1)'); }
    });
    expect(unreachable.devices).toBe(null);
    expect(unreachable.error).toBe('the browser bridge exited (code 1)');
  } finally {
    removeTemp(root);
  }
});

test('an extension that dropped off the bridge is put back before the session starts', async () => {
  const root = tempRoot('bro-browser-reconnect-');
  try {
    setClaudeBrowserEnabled(true, root);
    setPreferredBrowser('Google Chrome', root);
    const nudged = [];
    const state = await ensureBrowserConnected({
      claudePath: 'claude.exe',
      root,
      settleMs: 0,
      // Empty, then connected once the extension has been told to reconnect:
      // exactly the shape of a lapsed bridge session.
      probe: answering({ devices: [], error: null }, { devices: [{ deviceId: 'device-a' }], error: null }),
      nudge: (options) => { nudged.push(options.browser); return true; }
    });
    expect(state.connected).toBe(true);
    expect(state.nudged).toBe(true);
    expect(state.devices).toEqual([{ deviceId: 'device-a' }]);
    // The nudge has to land in the browser the user pinned, not the default.
    expect(nudged).toEqual(['Google Chrome']);
  } finally {
    removeTemp(root);
  }
});

test('a browser that answers straight away is never nudged', async () => {
  const root = tempRoot('bro-browser-connected-');
  try {
    setClaudeBrowserEnabled(true, root);
    let nudges = 0;
    const state = await ensureBrowserConnected({
      claudePath: 'claude.exe',
      root,
      probe: answering({ devices: [{ deviceId: 'device-a' }], error: null }),
      nudge: () => { nudges += 1; return true; }
    });
    expect(state.connected).toBe(true);
    expect(state.nudged).toBe(false);
    expect(nudges).toBe(0);
  } finally {
    removeTemp(root);
  }
});

test('a browser bro cannot even open is reported rather than waited on', async () => {
  const root = tempRoot('bro-browser-missing-');
  try {
    setClaudeBrowserEnabled(true, root);
    let probes = 0;
    const state = await ensureBrowserConnected({
      claudePath: 'claude.exe',
      root,
      settleMs: 10000, // never paid: the nudge fails, so there is no retry
      probe: async () => { probes += 1; return { devices: [], error: null }; },
      nudge: () => false
    });
    expect(state.connected).toBe(false);
    expect(probes).toBe(1);
    expect(state.reason).toBe('the bridge reports no connected browsers');
  } finally {
    removeTemp(root);
  }
});

test('the reconnect nudge is the extension url, opened in the chosen browser', () => {
  const calls = [];
  expect(nudgeExtensionReconnect({
    browser: 'Microsoft Edge',
    browserPath: 'C:/edge/msedge.exe',
    spawnBrowser: (command, args) => { calls.push({ command, args }); return { on() {}, unref() {} }; }
  })).toBe(true);
  expect(calls).toEqual([{ command: 'C:/edge/msedge.exe', args: [EXTENSION_RECONNECT_URL] }]);
  expect(EXTENSION_RECONNECT_URL).toBe('https://clau.de/chrome/reconnect');

  // No browser to open means no tab to open in it.
  expect(nudgeExtensionReconnect({ browserPath: null, spawnBrowser: () => ({}) })).toBe(false);
});

test('a session still launches when the browser stays unreachable, and says so', async () => {
  const root = tempRoot('bro-browser-warn-');
  const warnings = [];
  const error = console.error;
  console.error = (message) => warnings.push(String(message));
  try {
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, mode: 'main' }));
    const prepared = await prepareClaudeBrowser({
      root,
      claudePath: 'claude.exe',
      baseEnv: {},
      pipeNames: ['claude-mcp-browser-bridge-james'],
      verifyConnection: true,
      settleMs: 0,
      spawnBrowser: () => ({ on() {}, unref() {} }),
      ensureConnected: async () => ({ connected: false, devices: [], nudged: true, reason: 'the bridge reports no connected browsers' })
    });
    // The tools stay wired: the session is still usable, and Claude reports
    // its own connection state if the browser comes back mid-session.
    expect(prepared.args).toEqual(['--chrome']);
    expect(prepared.connection.connected).toBe(false);
    expect(warnings.join('\n')).toContain('No browser is connected to the Claude bridge');
    expect(warnings.join('\n')).toContain('bro browser reconnect');
  } finally {
    console.error = error;
    removeTemp(root);
  }
});

test('a normal launch skips the slow connection check and reconnect tab', async () => {
  const root = tempRoot('bro-browser-skip-');
  try {
    fs.writeFileSync(path.join(root, 'state.json'), JSON.stringify({ enabled: true, mode: 'main' }));
    let checks = 0;
    const prepared = await prepareClaudeBrowser({
      root,
      claudePath: 'claude.exe',
      baseEnv: {},
      pipeNames: ['claude-mcp-browser-bridge-james'],
      settleMs: 0,
      spawnBrowser: () => ({ on() {}, unref() {} }),
      ensureConnected: async () => { checks += 1; return { connected: true, devices: [] }; }
    });
    expect(checks).toBe(0);
    expect(prepared.connection).toBeUndefined();
  } finally {
    removeTemp(root);
  }
});
