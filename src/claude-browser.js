// One shared browser for every Claude Code login.
//
// The extension's native host is the pipe *server* on Windows, so any number
// of concurrent Claude sessions — any mix of CLAUDE_CONFIG_DIR profiles —
// connect to one browser at the same time. bro offers that in two modes:
//
// "main" (default): sessions attach to the user's normal Edge profile through
// the stock pipe. The store extension connects via Claude Desktop's native
// host when Desktop is installed, or Claude Code's own host otherwise; both
// serve the same bridge protocol (verified empirically — two profiles drove
// the main browser concurrently). bro's only job here is to scrub its own
// pipe-namespace environment off the session so it dials the stock pipe, and
// to start Edge when no bridge is up.
//
// "dedicated": a separate persistent Edge user-data directory with its own
// claude.ai sign-in, for keeping Claude's browsing out of the daily browser:
//   1. one Edge window shared by every Claude profile;
//   2. an unpacked copy of the installed extension prefers the Code host
//      (the store copy prefers Claude Desktop's);
//   3. a Bun preload moves Claude's browser named pipe into a stable
//      bro-specific namespace for both the native host and the CLI sessions,
//      so the dedicated browser never competes with the stock pipe.
// Anthropic's signed executable is never patched in either mode.

import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';
import { ensureClaude } from './proc.js';
import {
  CHROME_MCP_SERVER_NAME,
  browserFromUserAgent,
  chromeMcpConfig,
  describeConnectedBrowsers,
  listConnectedBrowsers,
  openChromeMcp
} from './chrome-mcp.js';
import {
  MCP_CHROME_SERVER_NAME,
  MCP_CHROME_URL,
  MCP_CHROME_EXTENSION_ID,
  ensureMcpChromeMultiClientFix,
  ensureMcpChromeNativeHost,
  mcpChromeEndpointStatus,
  probeMcpChrome,
  restartMcpChromeNativeHost,
  writeMcpChromeProfile
} from './mcp-chrome-server.js';

export { CHROME_MCP_SERVER_NAME, browserFromUserAgent };

export const CLAUDE_EXTENSION_ID = 'fcoeoabgfenejglbffodgkkbkcdhcgfn';
export const CLAUDE_CODE_NATIVE_HOST = 'com.anthropic.claude_code_browser_extension';

const PRELOAD_SOURCE = fileURLToPath(new URL('./claude-browser-preload.cjs', import.meta.url));
// Deliberately ignores CLAUDE_CONFIG_DIR from the surrounding shell, same as
// listClaudeLogins: a bro session launched from inside a pooled session must
// still treat this machine's own login as "local".
const LOCAL_CLAUDE_DIR = path.join(os.homedir(), '.claude');
const DEFAULT_CLAUDE_DIR = process.env.CLAUDE_CONFIG_DIR || LOCAL_CLAUDE_DIR;
const DEFAULT_POOL_DIR = process.env.CLAUDE_POOL_DIR || path.join(os.homedir(), '.claude-max-pool');
const DEFAULT_ACCOUNTS_DIR = path.join(DEFAULT_POOL_DIR, 'accounts');
const EDGE_REGISTRY_KEY = `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${CLAUDE_CODE_NATIVE_HOST}`;
const BRIDGE_PIPE_PREFIX = 'claude-mcp-browser-bridge-';

export function claudeBrowserRoot() {
  return process.env.BRO_CLAUDE_BROWSER_DIR || path.join(BRO_DIR, 'claude-browser');
}

function statePath(root = claudeBrowserRoot()) {
  return path.join(root, 'state.json');
}

function readState(root = claudeBrowserRoot()) {
  try {
    return JSON.parse(fs.readFileSync(statePath(root), 'utf8'));
  } catch {
    return {};
  }
}

export function claudeBrowserEnabled(root = claudeBrowserRoot()) {
  return readState(root).enabled === true;
}

export function claudeBrowserMode(root = claudeBrowserRoot()) {
  return readState(root).mode === 'dedicated' ? 'dedicated' : 'main';
}

export function browserBackend(root = claudeBrowserRoot()) {
  const backend = readState(root).backend;
  // Migrate the retired remote-debugging backend without making the user run
  // another command. Existing Anthropic-extension users keep their choice;
  // fresh `browser setup` commands select mcp-chrome below.
  return backend === 'mcp-chrome' || backend === 'devtools' ? 'mcp-chrome' : 'claude';
}

function writeState(patch, root = claudeBrowserRoot()) {
  fs.mkdirSync(root, { recursive: true });
  const next = { ...readState(root), ...patch, updatedAt: new Date().toISOString() };
  if (next.backend === 'mcp-chrome') {
    delete next.devtoolsPort;
    delete next.devtoolsProfile;
  }
  fs.writeFileSync(statePath(root), `${JSON.stringify(next, null, 2)}\n`);
}

// Every Chromium browser the Claude extension can live in. The display names
// are the ones identification reports back (see chrome-mcp.js), so a stored
// choice and a probed browser compare as plain strings.
export const CHROMIUM_BROWSERS = [
  { id: 'edge', name: 'Microsoft Edge', data: ['Microsoft', 'Edge', 'User Data'], exe: ['Microsoft', 'Edge', 'Application', 'msedge.exe'] },
  { id: 'chrome', name: 'Google Chrome', data: ['Google', 'Chrome', 'User Data'], exe: ['Google', 'Chrome', 'Application', 'chrome.exe'] },
  { id: 'brave', name: 'Brave', data: ['BraveSoftware', 'Brave-Browser', 'User Data'], exe: ['BraveSoftware', 'Brave-Browser', 'Application', 'brave.exe'] },
  { id: 'vivaldi', name: 'Vivaldi', data: ['Vivaldi', 'User Data'], exe: ['Vivaldi', 'Application', 'vivaldi.exe'] },
  { id: 'chromium', name: 'Chromium', data: ['Chromium', 'User Data'], exe: ['Chromium', 'Application', 'chrome.exe'] },
  { id: 'opera', name: 'Opera', data: ['Opera Software', 'Opera Stable'], exe: ['Programs', 'Opera', 'opera.exe'] }
];

// Accepts an id ("edge"), a display name, or any unambiguous prefix of either,
// so `bro browser use chrome` and `bro browser use "Google Chrome"` agree.
export function resolveBrowser(value) {
  const wanted = String(value || '').trim().toLowerCase();
  if (!wanted) return null;
  return CHROMIUM_BROWSERS.find((browser) => browser.id === wanted || browser.name.toLowerCase() === wanted)
    || CHROMIUM_BROWSERS.find((browser) => browser.id.startsWith(wanted) || browser.name.toLowerCase().startsWith(wanted))
    || null;
}

// The browser bro should drive. Empty means "whichever one is connected" —
// correct, and the common case, because a lone connected extension is
// auto-selected by the bridge with no pairing at all.
export function preferredBrowser(root = claudeBrowserRoot()) {
  return readState(root).browser || '';
}

export function setPreferredBrowser(name, root = claudeBrowserRoot()) {
  // A changed choice invalidates the device it resolved to last time.
  const previous = readState(root);
  const patch = { browser: name || '' };
  if (previous.browser !== patch.browser) Object.assign(patch, { pairedDeviceId: '', pairedBrowser: '' });
  writeState(patch, root);
}

// The menu's browser control turns the integration on by asking for a
// browser; setup is not a prerequisite, because a session starts the browser
// and Claude Code registers its own native host on first use.
export function setClaudeBrowserEnabled(enabled, root = claudeBrowserRoot()) {
  writeState({ enabled: Boolean(enabled) }, root);
}

export function pairedDevice(root = claudeBrowserRoot()) {
  const state = readState(root);
  return state.pairedDeviceId ? { deviceId: state.pairedDeviceId, browser: state.pairedBrowser || '' } : null;
}

export function setPairedDevice({ deviceId, browser }, root = claudeBrowserRoot()) {
  writeState({ pairedDeviceId: deviceId || '', pairedBrowser: browser || '' }, root);
}

const samePath = (a, b) => {
  const resolve = (value) => (process.platform === 'win32' ? path.resolve(value).toLowerCase() : path.resolve(value));
  return resolve(a) === resolve(b);
};

// The claude.ai account a login is signed in as. Empty for a login Claude
// Code has never authenticated.
export function accountUuidOf(configDir) {
  try {
    return String(JSON.parse(fs.readFileSync(path.join(configDir, '.claude.json'), 'utf8')).oauthAccount?.accountUuid || '').toLowerCase();
  } catch {
    return '';
  }
}

// The browser bridge is account-scoped: the extension only answers clients
// that identify as the claude.ai account it is signed into. The "owner" is
// the Claude login holding that account — the browser server always runs as
// it, whichever login the session itself uses. Defaults to this machine's
// own login, which is where the extension normally signs in.
export function browserOwnerConfigDir(root = claudeBrowserRoot(), {
  accountsDir = DEFAULT_ACCOUNTS_DIR,
  localDir = LOCAL_CLAUDE_DIR
} = {}) {
  const owner = readState(root).owner || '';
  return !owner || owner === 'local' ? localDir : path.join(accountsDir, owner);
}

export function setBrowserOwner(name, root = claudeBrowserRoot()) {
  writeState({ owner: name === 'local' ? 'local' : String(name || '') }, root);
}

// Every Claude session and the shared browser must agree on one pipe
// namespace, so the id is derived from the browser root rather than from any
// single login's CLAUDE_CONFIG_DIR.
export function sharedBridgeId(root = claudeBrowserRoot()) {
  const resolved = path.resolve(root);
  const stable = process.platform === 'win32' ? resolved.toLowerCase() : resolved;
  return crypto.createHash('sha256').update(stable).digest('hex').slice(0, 12);
}

export function sharedUserDataDir(root = claudeBrowserRoot()) {
  return path.join(root, 'edge-shared');
}

function legacyProfilesDir(root = claudeBrowserRoot()) {
  return path.join(root, 'edge-profiles');
}

// The extension's native host serves the bridge pipe, so its presence is the
// exact "browser is up and connectable" signal — no window management needed.
export function bridgePipeLive(id, { pipeNames } = {}) {
  if (process.platform !== 'win32' && !pipeNames) return false;
  try {
    const names = pipeNames || fs.readdirSync('\\\\.\\pipe\\');
    return names.some((name) => name.startsWith(`${BRIDGE_PIPE_PREFIX}bro-${id}-`));
  } catch {
    return false;
  }
}

// The stock pipe (no bro- namespace) is what the main Edge profile's
// extension serves, whether it connected through Claude Desktop's native host
// or Claude Code's own.
export function stockBridgePipeLive({ pipeNames } = {}) {
  if (process.platform !== 'win32' && !pipeNames) return false;
  try {
    const names = pipeNames || fs.readdirSync('\\\\.\\pipe\\');
    return names.some((name) => name.startsWith(BRIDGE_PIPE_PREFIX) && !name.startsWith(`${BRIDGE_PIPE_PREFIX}bro-`));
  } catch {
    return false;
  }
}

// In main mode a session must dial the stock pipe, so bro's own namespace —
// possibly inherited from the bro session this one is being launched from —
// has to come off the environment.
export function scrubBridgeEnv(env) {
  const options = String(env.BUN_OPTIONS || '')
    .split(/\s+/)
    .filter((arg) => arg && !(arg.startsWith('--preload=') && arg.includes('claude-browser-preload.cjs')))
    .join(' ');
  return { ...env, BRO_CLAUDE_BROWSER_ID: '', BUN_OPTIONS: options };
}

function bunPreloadArg(preloadPath) {
  // Forward slashes avoid the backslash escaping ambiguity of a Windows path.
  // BUN_OPTIONS keeps quotes as literal filename characters, so do not quote
  // this argument (bro's normal ~/.bro location has no spaces on Windows).
  return `--preload=${preloadPath.replace(/\\/g, '/')}`;
}

export function withBrowserBridgeEnv(env, { id, preloadPath }) {
  const prior = String(env.BUN_OPTIONS || '').trim();
  const preload = bunPreloadArg(preloadPath);
  // A session launched from inside another bro session already carries the
  // preload; only the id needs replacing then. Substring matching because the
  // preload path may itself contain spaces.
  const options = prior.includes(preload) ? prior : prior ? `${prior} ${preload}` : preload;
  return {
    ...env,
    BRO_CLAUDE_BROWSER_ID: id,
    BUN_OPTIONS: options
  };
}

export function extensionIdFromKey(key) {
  const hex = crypto.createHash('sha256').update(Buffer.from(String(key || ''), 'base64')).digest('hex').slice(0, 32);
  return [...hex].map((digit) => String.fromCharCode('a'.charCodeAt(0) + Number.parseInt(digit, 16))).join('');
}

const DESKTOP_HOST_ENTRY = '{name:"com.anthropic.claude_browser_extension",label:"Desktop"}';
const CODE_HOST_ENTRY = '{name:"com.anthropic.claude_code_browser_extension",label:"Claude Code"}';

export function patchClaudeExtensionWorker(source) {
  const desktopFirst = `${DESKTOP_HOST_ENTRY},${CODE_HOST_ENTRY}`;
  const codeFirst = `${CODE_HOST_ENTRY},${DESKTOP_HOST_ENTRY}`;
  if (source.includes(codeFirst)) return source;
  if (!source.includes(desktopFirst)) {
    throw new Error('The installed Claude extension changed its native-host bootstrap; bro could not create a safe profile copy.');
  }
  return source.replace(desktopFirst, codeFirst);
}

function edgeDataRoot(env = process.env) {
  return path.join(env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local'), 'Microsoft', 'Edge', 'User Data');
}

function versionParts(value) {
  return String(value || '').split(/[^0-9]+/).filter(Boolean).map(Number);
}

function compareVersions(left, right) {
  const a = versionParts(left);
  const b = versionParts(right);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const delta = (a[i] || 0) - (b[i] || 0);
    if (delta) return delta;
  }
  return 0;
}

export function findInstalledClaudeExtension({ userDataRoot = edgeDataRoot() } = {}) {
  if (!fs.existsSync(userDataRoot)) return null;
  const candidates = [];
  for (const profile of fs.readdirSync(userDataRoot, { withFileTypes: true })) {
    if (!profile.isDirectory() || (profile.name !== 'Default' && !profile.name.startsWith('Profile '))) continue;
    const versionsRoot = path.join(userDataRoot, profile.name, 'Extensions', CLAUDE_EXTENSION_ID);
    if (!fs.existsSync(versionsRoot)) continue;
    for (const versionDir of fs.readdirSync(versionsRoot, { withFileTypes: true })) {
      if (!versionDir.isDirectory()) continue;
      const dir = path.join(versionsRoot, versionDir.name);
      const manifestPath = path.join(dir, 'manifest.json');
      try {
        const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
        if (extensionIdFromKey(manifest.key) !== CLAUDE_EXTENSION_ID) continue;
        candidates.push({ dir, manifest, version: manifest.version || versionDir.name });
      } catch {
        /* ignore incomplete extension installs */
      }
    }
  }
  candidates.sort((a, b) => compareVersions(b.version, a.version));
  return candidates[0] || null;
}

// The bridge treats every browser whose extension connects as a selectable
// device, so a second install is exactly how sessions end up opening tabs in
// the wrong browser. Reported by display name, which is what a stored browser
// choice and a probed device are both spelled with.
export function browsersWithClaudeExtension(env = process.env) {
  return browsersWithExtension(CLAUDE_EXTENSION_ID, env);
}

function browsersWithExtension(extensionId, env = process.env) {
  const local = env.LOCALAPPDATA || path.join(os.homedir(), 'AppData', 'Local');
  return CHROMIUM_BROWSERS.filter((browser) => {
    try {
      return fs.readdirSync(path.join(local, ...browser.data), { withFileTypes: true }).some((entry) =>
        entry.isDirectory()
        && (entry.name === 'Default' || entry.name.startsWith('Profile '))
        && profileHasExtension(path.join(local, ...browser.data, entry.name), extensionId));
    } catch {
      return false; // that browser is not installed
    }
  }).map((browser) => browser.name);
}

function profileHasExtension(profileDir, extensionId) {
  if (fs.existsSync(path.join(profileDir, 'Extensions', extensionId))) return true;
  // Unpacked extensions live outside the browser data directory. Chromium
  // records their stable id and absolute path in Secure Preferences instead.
  try {
    const preferences = JSON.parse(fs.readFileSync(path.join(profileDir, 'Secure Preferences'), 'utf8'));
    const installed = preferences?.extensions?.settings?.[extensionId];
    return Boolean(installed && installed.state !== 0 && installed.path);
  } catch {
    return false;
  }
}

export function browsersWithMcpChromeExtension(env = process.env) {
  return browsersWithExtension(MCP_CHROME_EXTENSION_ID, env);
}

// Only worth saying when the choice is still open: once a browser is pinned,
// the extras are simply not the one bro connects to.
function warnAboutOtherBrowsers(root = claudeBrowserRoot()) {
  const chosen = preferredBrowser(root) || 'Microsoft Edge';
  const others = browsersWithClaudeExtension().filter((name) => name !== chosen);
  if (!others.length) return;
  if (pairedDevice(root)?.browser === chosen) {
    console.log(`\x1b[2mThe Claude extension is also in ${others.join(' and ')}; sessions stay pinned to ${chosen}.\x1b[0m`);
    return;
  }
  console.log(`⚠ The Claude extension is also installed in ${others.join(' and ')} — when that browser is open, sessions may use it instead of ${chosen}.`);
  console.log(`  Run "bro browser use ${resolveBrowser(chosen)?.id || 'edge'}" to pin ${chosen}, or remove the extension from the others.`);
}

export function findBrowserExecutable(browserId = 'edge', env = process.env) {
  const browser = resolveBrowser(browserId);
  if (!browser) return null;
  const roots = [env['PROGRAMFILES(X86)'], env.PROGRAMFILES, env.LOCALAPPDATA].filter(Boolean);
  const candidates = [
    browser.id === 'edge' ? env.BRO_EDGE_PATH : null,
    ...roots.map((root) => path.join(root, ...browser.exe))
  ].filter(Boolean);
  return candidates.find((candidate) => fs.existsSync(candidate)) || null;
}

export function findEdgeExecutable(env = process.env) {
  return findBrowserExecutable('edge', env);
}

function serviceWorkerPath(extensionDir) {
  const loader = fs.readFileSync(path.join(extensionDir, 'service-worker-loader.js'), 'utf8');
  const match = /import\s+['"]\.\/([^'"]+)['"]/.exec(loader);
  if (!match) throw new Error('Could not locate the Claude extension service worker.');
  return path.join(extensionDir, match[1]);
}

function safeRemoveGeneratedTemp(extensionRoot, candidate) {
  const resolvedRoot = path.resolve(extensionRoot);
  const resolved = path.resolve(candidate);
  if (path.dirname(resolved) !== resolvedRoot || !path.basename(resolved).startsWith('.building-')) return;
  fs.rmSync(resolved, { recursive: true, force: true });
}

export function ensurePatchedExtension({ root = claudeBrowserRoot(), installed = findInstalledClaudeExtension() } = {}) {
  if (!installed) {
    throw new Error(
      `Claude in Chrome (${CLAUDE_EXTENSION_ID}) is not installed in Edge. Install it once, then run "bro browser setup" again.`
    );
  }
  const sourceWorker = serviceWorkerPath(installed.dir);
  const sourceText = fs.readFileSync(sourceWorker, 'utf8');
  const fingerprint = crypto.createHash('sha256').update(sourceText).digest('hex').slice(0, 10);
  const extensionRoot = path.join(root, 'extensions');
  const destination = path.join(extensionRoot, `${installed.version}-${fingerprint}`);
  const markerPath = path.join(destination, '.bro-profile-extension.json');
  if (fs.existsSync(markerPath)) return { dir: destination, version: installed.version };

  fs.mkdirSync(extensionRoot, { recursive: true });
  const building = path.join(extensionRoot, `.building-${process.pid}-${Date.now()}`);
  try {
    fs.cpSync(installed.dir, building, { recursive: true, errorOnExist: true });
    const worker = serviceWorkerPath(building);
    fs.writeFileSync(worker, patchClaudeExtensionWorker(fs.readFileSync(worker, 'utf8')));
    fs.writeFileSync(
      path.join(building, '.bro-profile-extension.json'),
      `${JSON.stringify({ source: installed.dir, version: installed.version, fingerprint }, null, 2)}\n`
    );
    if (fs.existsSync(destination)) {
      throw new Error(`Claude browser extension destination is incomplete: ${destination}`);
    }
    fs.renameSync(building, destination);
  } catch (error) {
    safeRemoveGeneratedTemp(extensionRoot, building);
    throw error;
  }
  return { dir: destination, version: installed.version };
}

function escapeBatchValue(value) {
  return String(value).replace(/%/g, '%%').replace(/\r?\n/g, '');
}

function ensureNativeHost({ root, claudePath, preloadPath, extensionId = CLAUDE_EXTENSION_ID, register = true }) {
  const nativeRoot = path.join(root, 'native-host');
  const wrapperPath = path.join(nativeRoot, 'claude-code-browser-host.cmd');
  const manifestPath = path.join(nativeRoot, `${CLAUDE_CODE_NATIVE_HOST}.json`);
  fs.mkdirSync(nativeRoot, { recursive: true });

  const preloadArg = bunPreloadArg(preloadPath);
  const wrapper = [
    '@echo off',
    'setlocal',
    `set BUN_OPTIONS=%BUN_OPTIONS% ${escapeBatchValue(preloadArg)}`,
    `"${escapeBatchValue(claudePath)}" --chrome-native-host`,
    ''
  ].join('\r\n');
  fs.writeFileSync(wrapperPath, wrapper);
  fs.writeFileSync(manifestPath, `${JSON.stringify({
    name: CLAUDE_CODE_NATIVE_HOST,
    description: 'Claude Code Browser Extension Native Host (bro shared browser)',
    path: wrapperPath,
    type: 'stdio',
    allowed_origins: [`chrome-extension://${extensionId}/`]
  }, null, 2)}\n`);

  if (register) {
    const result = spawnSync('reg.exe', [
      'add', EDGE_REGISTRY_KEY, '/ve', '/t', 'REG_SZ', '/d', manifestPath, '/f'
    ], { encoding: 'utf8', windowsHide: true });
    if (result.status !== 0) {
      throw new Error(`Could not register Edge's Claude native host: ${(result.stderr || result.stdout || '').trim()}`);
    }
  }
  return { wrapperPath, manifestPath };
}

export function ensureClaudeBrowserInfrastructure({
  root = claudeBrowserRoot(),
  claudePath,
  installedExtension,
  register = true
} = {}) {
  if (process.platform !== 'win32') throw new Error('The shared Claude browser currently requires Windows + Microsoft Edge.');
  const edgePath = findEdgeExecutable();
  if (!edgePath) throw new Error('Microsoft Edge was not found. Set BRO_EDGE_PATH to msedge.exe and retry.');
  if (!claudePath) throw new Error('Claude Code executable path is required for browser setup.');

  fs.mkdirSync(root, { recursive: true });
  const preloadPath = path.join(root, 'claude-browser-preload.cjs');
  fs.copyFileSync(PRELOAD_SOURCE, preloadPath);
  const extension = ensurePatchedExtension({ root, installed: installedExtension || findInstalledClaudeExtension() });
  const nativeHost = ensureNativeHost({ root, claudePath, preloadPath, register });
  return { root, edgePath, preloadPath, extension, nativeHost };
}

// Earlier bro versions kept one Edge profile per Claude login. Carry the most
// recently used one over as the shared browser so its claude.ai sign-in
// survives the migration. While that Edge is still running its directory is
// locked; failing loudly (instead of quietly creating a fresh profile, which
// would forfeit the carry-over forever) keeps the migration retryable.
export function adoptLegacyEdgeProfile(root = claudeBrowserRoot()) {
  const shared = sharedUserDataDir(root);
  if (fs.existsSync(shared)) return null;
  let newest = null;
  try {
    for (const entry of fs.readdirSync(legacyProfilesDir(root), { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const dir = path.join(legacyProfilesDir(root), entry.name);
      let mtime;
      try {
        mtime = fs.statSync(path.join(dir, 'Local State')).mtimeMs;
      } catch {
        continue;
      }
      if (!newest || mtime > newest.mtime) newest = { name: entry.name, dir, mtime };
    }
  } catch {
    return null;
  }
  if (!newest) return null;
  try {
    fs.renameSync(newest.dir, shared);
    return { name: newest.name };
  } catch {
    throw new Error(
      `Your previous per-login browser ("${newest.name}") is still open, so its claude.ai sign-in cannot be carried over yet. ` +
      'Close every old Claude Edge window and retry — or run "bro browser clean" first to start the shared browser fresh.'
    );
  }
}

function wait(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

export async function openClaudeBrowser({
  claudePath,
  root = claudeBrowserRoot(),
  signIn = false,
  reuseIfRunning = true,
  // With no window allowed on screen, a browser that is already up is still
  // joined; one that is not stays down, and the session runs without it.
  startIfDown = true,
  infrastructure,
  spawnBrowser = spawn,
  settleMs
} = {}) {
  const setup = infrastructure || ensureClaudeBrowserInfrastructure({ root, claudePath });
  const id = sharedBridgeId(root);
  const userDataDir = sharedUserDataDir(root);

  if (reuseIfRunning && bridgePipeLive(id)) {
    return { id, userDataDir, firstRun: false, reused: true, adopted: null, ...setup };
  }
  if (!startIfDown) {
    return { id, userDataDir, firstRun: false, reused: false, started: false, adopted: null, ...setup };
  }

  const adopted = adoptLegacyEdgeProfile(root);
  const firstRun = !fs.existsSync(path.join(userDataDir, 'Local State'));
  fs.mkdirSync(userDataDir, { recursive: true });

  const args = [
    `--user-data-dir=${userDataDir}`,
    '--profile-directory=Default',
    `--load-extension=${setup.extension.dir}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-sync',
    '--disable-features=msEdgeFirstRunExperience'
  ];
  // When the shared Edge is already running, a URL becomes a tab in the
  // existing window instead of spawning an empty extra window.
  if (signIn || firstRun) args.push('https://claude.ai/');

  let edgeEnv = {
    ...process.env,
    BRO_CLAUDE_BROWSER_ID: id,
    CLAUDE_CONFIG_DIR: path.resolve(DEFAULT_CLAUDE_DIR)
  };
  delete edgeEnv.CLAUDECODE;
  // Claude Code may rewrite the global native-host registry entry when a new
  // session enables Chrome. Edge therefore carries the preload itself too: an
  // Anthropic-generated wrapper still inherits the shared namespace.
  edgeEnv = withBrowserBridgeEnv(edgeEnv, { id, preloadPath: setup.preloadPath });

  const child = spawnBrowser(setup.edgePath, args, {
    env: edgeEnv,
    detached: true,
    stdio: 'ignore'
  });
  child.on?.('error', () => {});
  child.unref?.();
  await wait(settleMs ?? (firstRun ? 1200 : 500));
  return { id, userDataDir, firstRun, reused: false, adopted, ...setup };
}

export function openMainBrowser({
  spawnBrowser = spawn,
  browser = preferredBrowser() || 'Microsoft Edge',
  browserPath = findBrowserExecutable(resolveBrowser(browser)?.id || 'edge'),
  url
} = {}) {
  if (!browserPath) {
    const hint = resolveBrowser(browser)?.id === 'edge' ? ' Set BRO_EDGE_PATH to msedge.exe and retry.' : '';
    throw new Error(`${browser} was not found.${hint}`);
  }
  const child = spawnBrowser(browserPath, url ? [url] : [], { detached: true, stdio: 'ignore' });
  child.on?.('error', () => {});
  child.unref?.();
}

// The extension's own recovery hook, and the only one that needs no clicking.
// Navigating any tab to it makes the service worker drop its native port,
// reset, re-run both connection attempts — the native host *and* the
// per-account room on Anthropic's bridge that the browser tools actually read
// — and then close the tab it was handed. Everything else that reconnects an
// extension (the Connect prompt, reloading it in the browser's extension page)
// needs a human, so this is what bro fires when the bridge reports nothing.
export const EXTENSION_RECONNECT_URL = 'https://clau.de/chrome/reconnect';

export function nudgeExtensionReconnect({
  spawnBrowser = spawn,
  browser = preferredBrowser() || 'Microsoft Edge',
  browserPath = findBrowserExecutable(resolveBrowser(browser)?.id || 'edge')
} = {}) {
  if (!browserPath) return false;
  openMainBrowser({ spawnBrowser, browser, browserPath, url: EXTENSION_RECONNECT_URL });
  return true;
}

export const openMainEdge = ({ spawnBrowser = spawn, edgePath = findEdgeExecutable() } = {}) =>
  openMainBrowser({ spawnBrowser, browser: 'Microsoft Edge', browserPath: edgePath });

// Claude Code only wires its own browser tools when the session authenticates
// with a claude.ai account, and setting ANTHROPIC_AUTH_TOKEN (or an API key)
// ends that — quietly, with --chrome accepted and then ignored. Every bro
// route except a native login lands there: third-party providers, the account
// pool's local proxy, the Codex bridge. Those sessions reach the same browser
// through an explicitly configured MCP server instead.
export function usesThirdPartyAuth(env = process.env) {
  return Boolean(env.ANTHROPIC_AUTH_TOKEN || env.ANTHROPIC_API_KEY);
}

// The MCP config handed to bridged sessions. Two fixed files rather than one
// rewritten per launch, so concurrent sessions wanting different permission
// modes never race over the same bytes. The owner login is baked into the
// server's env — it is global state, so every session shares the same file.
export function writeChromeMcpConfig({
  root = claudeBrowserRoot(),
  claudePath,
  skipPermissions = false,
  ownerConfigDir = browserOwnerConfigDir(root)
} = {}) {
  const file = path.join(root, skipPermissions ? 'chrome-mcp.json' : 'chrome-mcp-ask.json');
  const body = `${JSON.stringify(chromeMcpConfig({ claudePath, skipPermissions, ownerConfigDir }), null, 2)}\n`;
  fs.mkdirSync(root, { recursive: true });
  // Same content on every launch; skip the write when it already matches so a
  // running session's config file is never truncated underneath it.
  if (readFileOrNull(file) !== body) fs.writeFileSync(file, body);
  return file;
}

function readFileOrNull(file) {
  try {
    return fs.readFileSync(file, 'utf8');
  } catch {
    return null;
  }
}

// Passed as a file rather than inline: the proxy route spawns through a .cmd
// shim, whose command line is a single string, and a multi-line argument does
// not survive that. A path has no newlines.
export function writeChromeSystemPrompt({ root = claudeBrowserRoot() } = {}) {
  const file = path.join(root, 'chrome-prompt.md');
  const body = `${CHROME_MCP_SYSTEM_PROMPT}\n`;
  fs.mkdirSync(root, { recursive: true });
  if (readFileOrNull(file) !== body) fs.writeFileSync(file, body);
  return file;
}

// Non-Claude models have none of Claude Code's built-in browser briefing, and
// under bro the tools answer to a different prefix. Both facts fit in a note.
export const CHROME_MCP_SYSTEM_PROMPT = [
  '# Browser automation',
  '',
  `You can drive the user's real browser with the mcp__${CHROME_MCP_SERVER_NAME}__* tools — it is their own`,
  'signed-in browser, not a sandbox, so treat pages as live and never trigger alert/confirm',
  'dialogs (they block the extension until dismissed by hand).',
  '',
  `Call mcp__${CHROME_MCP_SERVER_NAME}__tabs_context_mcp once before anything else to learn which tabs exist.`,
  `Open your own tab with mcp__${CHROME_MCP_SERVER_NAME}__tabs_create_mcp rather than reusing the user's, and close`,
  `it with mcp__${CHROME_MCP_SERVER_NAME}__tabs_close_mcp when you are done. Prefer mcp__${CHROME_MCP_SERVER_NAME}__browser_batch to`,
  'run several actions in one call. If the browser stops responding after two or three',
  'attempts, stop and say so instead of retrying.'
].join('\n');

// The environment the browser server runs in, whoever asks. Identical to what
// chromeMcpServer bakes into a bridged session's config, so a probe answers
// exactly what that session's own server would.
function ownerBridgeEnv(env, ownerConfigDir) {
  return { ...scrubBridgeEnv(env), CLAUDE_CONFIG_DIR: ownerConfigDir, CLAUDE_CODE_ENABLE_CFC: 'true' };
}

// The only honest readiness signal. A live pipe proves nothing — Claude
// Desktop's native host answers the same host name and holds the same pipe —
// and the browser tools do not read that pipe at all: they join a per-account
// room on wss://bridge.claudeusercontent.com and ask which extensions are in
// it. So "is the browser reachable?" has exactly one answer, and this is it.
export async function probeConnectedBrowsers({
  claudePath,
  root = claudeBrowserRoot(),
  env = process.env,
  ownerConfigDir = browserOwnerConfigDir(root),
  timeoutMs = 8000,
  openBridge = openChromeMcp
} = {}) {
  let client;
  try {
    client = await openBridge({ claudePath, env: ownerBridgeEnv(env, ownerConfigDir), timeoutMs });
  } catch (error) {
    return { devices: null, error: error.message };
  }
  try {
    return { devices: await listConnectedBrowsers(client), error: null };
  } catch (error) {
    return { devices: null, error: error.message };
  } finally {
    client.close();
  }
}

// Ask, and if nobody answers, fix it and ask again. An extension whose bridge
// session lapsed stays lapsed until something tells it to reconnect — it does
// not retry on its own, and a session launched next to it simply reports that
// its browser tools are unavailable. One reconnect nudge covers that entire
// class of failure without the user noticing there was one.
export async function ensureBrowserConnected({
  claudePath,
  root = claudeBrowserRoot(),
  env = process.env,
  spawnBrowser = spawn,
  browser = preferredBrowser(root) || 'Microsoft Edge',
  probe = probeConnectedBrowsers,
  nudge = nudgeExtensionReconnect,
  timeoutMs = 8000,
  settleMs = 2500,
  // Only the retry after a nudge is worth waiting for; a third attempt just
  // delays a session that is going to launch without the browser anyway.
  attempts = 2
} = {}) {
  let last = { devices: null, error: null };
  for (let attempt = 0; attempt < attempts; attempt++) {
    last = await probe({ claudePath, root, env, timeoutMs });
    if (last.devices?.length) return { connected: true, devices: last.devices, nudged: attempt > 0 };
    if (attempt + 1 >= attempts) break;
    // Nothing to reconnect if the browser holding the extension is not
    // installed — that is a setup problem, and another wait will not fix it.
    if (!nudge({ spawnBrowser, browser })) break;
    await wait(settleMs);
  }
  return {
    connected: false,
    devices: last.devices || [],
    nudged: attempts > 1,
    reason: last.error || 'the bridge reports no connected browsers'
  };
}

// What to say when the bridge stays empty. The cause is almost always the same
// one and it is not something bro can do for the user: the extension carries
// its own claude.ai sign-in, and the bridge room it joins is keyed by that
// account, so a lapsed extension session is invisible to every login bro has.
export function browserDisconnectedNotice(state, root = claudeBrowserRoot()) {
  const browser = preferredBrowser(root) || 'your browser';
  const owner = listClaudeLogins().find((login) => samePath(login.configDir, browserOwnerConfigDir(root)));
  const account = owner?.email ? ` as ${owner.email}` : '';
  return [
    `  ⚠ No browser is connected to the Claude bridge (${state.reason}) — this session's browser tools will not answer.`,
    `    Open ${browser}, sign the Claude extension in to claude.ai${account}, then run "bro browser reconnect".`
  ].join('\n');
}

// Called on every Claude session launch: connects the session to the shared
// browser (starting it when needed) and returns the env and arguments that
// point the session's browser tools at it. Never blocks a launch — a broken
// browser setup degrades to a plain session with a warning.
export async function prepareClaudeBrowser({
  claudePath,
  baseEnv = process.env,
  root = claudeBrowserRoot(),
  spawnBrowser = spawn,
  skipPermissions = false,
  bridged,
  // A headless run still gets the browser tools and still attaches to a
  // browser that is already up — it just will not raise a window on someone's
  // screen to get one. A cron job that pops open Edge is a bug.
  autoStart = true,
  // Keep the normal launch path cheap and quiet. A live native-host pipe is
  // enough to wire the tools; actively probing the account-scoped bridge can
  // take seconds and its recovery path opens a reconnect tab. Diagnostics and
  // repair belong to `bro browser status` / `bro browser reconnect`, while a
  // session can safely start with temporarily unavailable browser tools.
  verifyConnection = false,
  ensureConnected = ensureBrowserConnected,
  pipeNames,
  settleMs,
  ownerConfigDir
} = {}) {
  if (!claudeBrowserEnabled(root)) return null;
  const sessionDir = baseEnv.CLAUDE_CONFIG_DIR || LOCAL_CLAUDE_DIR;
  if (browserBackend(root) === 'mcp-chrome') {
    const { configPath, promptPath, skillPath } = writeMcpChromeProfile({ configDir: sessionDir });
    let connection = await mcpChromeEndpointStatus();
    if (!connection.connected && autoStart) {
      try {
        openMainBrowser({ spawnBrowser, browser: preferredBrowser(root) || 'Microsoft Edge' });
        const deadline = Date.now() + (settleMs ?? 5000);
        while (Date.now() < deadline && !connection.connected) {
          await wait(250);
          connection = await mcpChromeEndpointStatus();
        }
      } catch { /* keep launch non-blocking */ }
    }
    if (!connection.connected && autoStart) {
      console.error(`  ⚠ mcp-chrome is not answering at ${MCP_CHROME_URL}; the session will still launch with mcp__${MCP_CHROME_SERVER_NAME}__* configured.`);
    }
    return {
      mode: 'main',
      backend: 'mcp-chrome',
      bridged: true,
      connection,
      configPath,
      promptPath,
      skillPath,
      env: { ...scrubBridgeEnv(baseEnv), CLAUDE_CODE_ENABLE_CFC: 'false' },
      args: ['--no-chrome', '--mcp-config', configPath, '--append-system-prompt-file', promptPath]
    };
  }
  const ownerDir = ownerConfigDir || browserOwnerConfigDir(root);
  // The bridge only answers the account the extension is signed into, so a
  // session on any other login (an account-pool profile, say) cannot use
  // Claude Code's built-in wiring — it would identify as its own account and
  // see zero browsers. Those sessions get the browser server instead, which
  // runs as the owner login.
  const foreignLogin = !samePath(sessionDir, ownerDir)
    && accountUuidOf(sessionDir) !== accountUuidOf(ownerDir);
  const viaMcp = bridged ?? (usesThirdPartyAuth(baseEnv) || foreignLogin);
  // Claude Code may auto-wire its own (account-bound, therefore broken)
  // browser tools next to bro's working ones; a foreign login switches that
  // off so the model only ever sees the tools that answer.
  const sessionEnvPatch = viaMcp && !usesThirdPartyAuth(baseEnv)
    ? { CLAUDE_CODE_ENABLE_CFC: 'false' }
    : {};

  // A bridged session configures the browser server itself, so Claude Code's
  // own --chrome would only add a flag it has already decided to ignore.
  const wiring = () => {
    if (!viaMcp) return { args: ['--chrome'] };
    try {
      const configPath = writeChromeMcpConfig({ root, claudePath, skipPermissions, ownerConfigDir: ownerDir });
      const promptPath = writeChromeSystemPrompt({ root });
      return { args: ['--mcp-config', configPath, '--append-system-prompt-file', promptPath], configPath, promptPath };
    } catch (error) {
      console.error(`  ⚠ Browser tools unavailable (${error.message}) — launching without them.`);
      return null;
    }
  };

  if (claudeBrowserMode(root) === 'main') {
    const wired = wiring();
    if (!wired) return null;
    const env = { ...scrubBridgeEnv(baseEnv), ...sessionEnvPatch };
    if (autoStart && !stockBridgePipeLive({ pipeNames })) {
      // Best effort only: with no bridge up, the most likely cause is that
      // the browser is closed. If it cannot be started the session still
      // launches, and Claude reports its own connection state.
      try {
        openMainBrowser({ spawnBrowser });
        await wait(settleMs ?? 1500);
      } catch {
        /* the session proceeds without a browser */
      }
    }
    // Explicit callers may request the slower end-to-end check. Normal Claude
    // launches skip it so they never open a reconnect tab just to start.
    let connection;
    if (verifyConnection) {
      connection = await ensureConnected({ claudePath, root, env, spawnBrowser });
      if (!connection.connected) console.error(browserDisconnectedNotice(connection, root));
    }
    return { mode: 'main', bridged: viaMcp, env, connection, ...wired };
  }

  try {
    const wired = wiring();
    if (!wired) return null;
    const opened = await openClaudeBrowser({ claudePath, root, spawnBrowser, settleMs, startIfDown: autoStart });
    return {
      mode: 'dedicated',
      bridged: viaMcp,
      ...opened,
      ...wired,
      env: { ...withBrowserBridgeEnv(baseEnv, { id: opened.id, preloadPath: opened.preloadPath }), ...sessionEnvPatch }
    };
  } catch (error) {
    console.error(`  ⚠ Claude browser unavailable (${error.message}) — launching without it.`);
    return null;
  }
}

export function listClaudeLogins({
  accountsDir = DEFAULT_ACCOUNTS_DIR,
  // Deliberately ignore CLAUDE_CONFIG_DIR from the surrounding shell: running
  // this from inside a pooled session must not present that pool profile as
  // the "local" login.
  defaultConfigDir = path.join(os.homedir(), '.claude')
} = {}) {
  const login = (name, configDir, local) => {
    let authenticated = false;
    let email = '';
    try {
      authenticated = Boolean(JSON.parse(fs.readFileSync(path.join(configDir, '.credentials.json'), 'utf8')).claudeAiOauth?.accessToken);
    } catch { /* logged out */ }
    try {
      email = String(JSON.parse(fs.readFileSync(path.join(configDir, '.claude.json'), 'utf8')).oauthAccount?.emailAddress || '');
    } catch { /* never used */ }
    return { name, configDir, local, authenticated, email };
  };
  const logins = [login('local', defaultConfigDir, true)];
  try {
    for (const entry of fs.readdirSync(accountsDir, { withFileTypes: true })) {
      if (entry.isDirectory()) logins.push(login(entry.name, path.join(accountsDir, entry.name), false));
    }
  } catch {
    /* no account pool on this machine */
  }
  return logins;
}

// Which browser a session connects to is settled inside the bridge: a lone
// connected extension is auto-selected, and beyond that the deciding vote is
// the device id persisted in a login's .claude.json. Writing it there is
// therefore how a browser choice becomes binding — for bro's own sessions and
// for plain `claude` alike, since both read the same file.
export function pinPairedDevice({ configDir, deviceId, browser }) {
  const file = path.join(configDir, '.claude.json');
  let config;
  try {
    config = JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return false; // a login Claude Code has never written to has nothing to pin
  }
  if (!config || typeof config !== 'object' || Array.isArray(config)) return false;
  // No device means "stop pinning": leave the file as if it had never been
  // pinned rather than pinning it to nothing.
  if (!deviceId) {
    if (!config.chromeExtension) return false;
    delete config.chromeExtension;
  } else {
    if (config.chromeExtension?.pairedDeviceId === deviceId) return true;
    config.chromeExtension = { ...(config.chromeExtension || {}), pairedDeviceId: deviceId, pairedDeviceName: browser };
  }
  fs.writeFileSync(file, JSON.stringify(config, null, 2));
  return true;
}

// Open the bridge for bro's own questions. Sessions get their browser tools
// through prepareClaudeBrowser; this is the same server, driven directly, so
// "which browsers are connected?" costs milliseconds instead of a model turn.
async function withChromeMcp(root, run) {
  const { claude } = ensureClaude();
  // These commands report the bridge's answer themselves — and one of them is
  // the reconnect — so the launch-time self-heal would only talk over them.
  const browser = await prepareClaudeBrowser({ claudePath: claude, baseEnv: process.env, root, bridged: true, verifyConnection: false });
  if (!browser) {
    console.error('Claude browser integration is disabled. Run "bro browser setup" first.');
    return 1;
  }
  let client;
  try {
    // bro's own questions go through the owner login as well — any other
    // account would be answered with an empty browser list.
    client = await openChromeMcp({
      claudePath: claude,
      env: { ...browser.env, CLAUDE_CONFIG_DIR: browserOwnerConfigDir(root) }
    });
  } catch (error) {
    console.error(`✗ Could not reach the browser bridge: ${error.message}`);
    return 1;
  }
  try {
    return await run(client);
  } catch (error) {
    // The bridge is a live connection to a browser the user is also using; a
    // closed window mid-question is ordinary, not a crash.
    console.error(`✗ The browser bridge stopped answering: ${error.message}`);
    return 1;
  } finally {
    client.close();
  }
}

function reportNoBrowsers(root) {
  const chosen = preferredBrowser(root) || 'your browser';
  console.error(`✗ No browser is connected. Open ${chosen}, check that the Claude extension is enabled, and retry.`);
  console.error('  "bro browser reconnect" puts a lapsed extension back on the bridge; if that does not take, the');
  console.error('  extension itself is signed out — its claude.ai sign-in is separate from every login bro has.');
}

// A live end-to-end check, driven straight through the bridge: list the
// connected browsers, name each one by opening one throwaway tab in it, and
// say which one sessions will actually get.
async function runBrowserSelfTest(root) {
  return withChromeMcp(root, async (client) => {
    console.log('Asking the browser bridge which browsers are connected…');
    const devices = await describeConnectedBrowsers(client);
    if (!devices.length) {
      reportNoBrowsers(root);
      return 1;
    }

    const chosen = preferredBrowser(root);
    const pinned = pairedDevice(root);
    for (const device of devices) {
      const name = device.browser || `unidentified (${device.reason || 'no evidence'})`;
      const marks = [
        device.deviceId === pinned?.deviceId ? 'pinned' : null,
        device.isLocal ? null : 'remote',
        chosen && device.browser === chosen ? 'chosen' : null
      ].filter(Boolean);
      console.log(`  ${name}${marks.length ? `  \x1b[2m(${marks.join(', ')})\x1b[0m` : ''}`);
      if (device.evidence) console.log(`    \x1b[2m${device.evidence}\x1b[0m`);
    }

    // With one browser connected the bridge auto-selects it, so that is the
    // answer whether or not anything was ever pinned.
    const serving = devices.length === 1
      ? devices[0]
      : devices.find((device) => device.deviceId === pinned?.deviceId)
        || devices.find((device) => device.browser === chosen);

    if (!serving) {
      console.error(`✗ ${devices.length} browsers are connected and none is pinned — sessions will stop and ask which to use.`);
      console.error('  Run "bro browser use <edge|chrome|brave|…>" to settle it once.');
      return 1;
    }
    const wrong = chosen && serving.browser && serving.browser !== chosen;
    if (wrong) {
      console.error(`✗ Sessions will use ${serving.browser}, but ${chosen} is the chosen browser.`);
      console.error(`  Run "bro browser use ${resolveBrowser(chosen)?.id || chosen}" with ${chosen} open.`);
      return 1;
    }
    console.log(`✓ Sessions open tabs in ${serving.browser || 'the connected browser'}.`);
    warnAboutOtherBrowsers(root);
    return 0;
  });
}

// Every login bro can launch, checked the way its own session would actually
// reach the browser — because the two routes fail independently. A login on
// the owner's account talks to the bridge as itself; any other login talks to
// it through the browser server that runs as the owner. Asking only one of
// them is how "it works for me" gets mistaken for "it works".
async function runBrowserVerify(root) {
  if (!claudeBrowserEnabled(root)) {
    console.error('Claude browser integration is disabled. Run "bro browser setup" first.');
    return 1;
  }
  const { claude } = ensureClaude();
  const ownerDir = browserOwnerConfigDir(root);
  const logins = listClaudeLogins();
  console.log(`Checking ${logins.length} logins against the browser bridge…\n`);

  let failed = 0;
  for (const login of logins) {
    const wiring = await prepareClaudeBrowser({
      claudePath: claude,
      root,
      baseEnv: { ...process.env, CLAUDE_CONFIG_DIR: login.configDir },
      // Neither raise a window nor self-heal here: this reports the state as
      // it is, and a nudge partway through would make each row a different
      // question from the last.
      autoStart: false,
      verifyConnection: false
    });
    if (!wiring) {
      console.error(`  ✗ ${loginLabel(login)} — bro could not wire the browser at all.`);
      failed += 1;
      continue;
    }
    const route = wiring.bridged ? `mcp__${CHROME_MCP_SERVER_NAME}__*` : '--chrome';
    const { devices, error } = await probeConnectedBrowsers({
      claudePath: claude,
      root,
      env: wiring.env,
      ownerConfigDir: wiring.bridged ? ownerDir : login.configDir
    });
    const detail = `\x1b[2m${route}${wiring.bridged ? ` as ${path.basename(ownerDir)}` : ''}\x1b[0m`;
    if (error) {
      console.error(`  ✗ ${loginLabel(login)} — ${error}  ${detail}`);
      failed += 1;
    } else if (!devices.length) {
      console.error(`  ✗ ${loginLabel(login)} — no browser answers  ${detail}`);
      failed += 1;
    } else {
      console.log(`  ✓ ${loginLabel(login)} — ${devices.length === 1 ? '1 browser' : `${devices.length} browsers`}  ${detail}`);
    }
  }

  if (!failed) {
    console.log(`\n✓ All ${logins.length} logins reach the browser.`);
    return 0;
  }
  console.error(`\n✗ ${failed} of ${logins.length} logins cannot reach the browser.`);
  // One shared browser, one shared cause: when nothing answers anywhere, it is
  // the extension that is off the bridge, not any single login's wiring.
  if (failed === logins.length) console.error(browserDisconnectedNotice({ reason: 'the bridge reports no connected browsers' }, root).replace(/^ {2}/gm, ''));
  return 1;
}

// Pin a browser: find the connected extension that belongs to it, remember it,
// and write it into every Claude login so the bridge stops guessing.
async function runBrowserUse(root, requested) {
  const browser = resolveBrowser(requested);
  if (requested && !browser) {
    console.error(`Unknown browser "${requested}". Choices: ${CHROMIUM_BROWSERS.map((entry) => entry.id).join(', ')}`);
    return 1;
  }
  if (!browser) {
    const current = preferredBrowser(root);
    console.log(`Browser: ${current || 'whichever one is connected'}`);
    console.log(`Usage: bro browser use <${CHROMIUM_BROWSERS.map((entry) => entry.id).join('|')}|auto>`);
    return 0;
  }

  return withChromeMcp(root, async (client) => {
    console.log(`Looking for ${browser.name} among the connected browsers…`);
    const devices = await describeConnectedBrowsers(client);
    if (!devices.length) {
      reportNoBrowsers(root);
      return 1;
    }
    const match = devices.find((device) => device.browser === browser.name);
    if (!match) {
      const seen = devices.map((device) => device.browser || 'an unidentified browser').join(', ');
      console.error(`✗ ${browser.name} is not connected. Connected right now: ${seen}.`);
      console.error(`  Open ${browser.name} with the Claude extension enabled, then rerun this.`);
      return 1;
    }

    setPreferredBrowser(browser.name, root);
    setPairedDevice({ deviceId: match.deviceId, browser: browser.name }, root);
    await client.callTool('select_browser', { deviceId: match.deviceId }).catch(() => {});

    const pinned = listClaudeLogins()
      .filter((login) => pinPairedDevice({ configDir: login.configDir, deviceId: match.deviceId, browser: browser.name }))
      .map((login) => login.name);
    console.log(`✓ Sessions now use ${browser.name} (device ${match.deviceId.slice(0, 8)}).`);
    if (pinned.length) console.log(`  Pinned for ${pinned.length === 1 ? 'login' : 'logins'}: ${pinned.join(', ')}`);
    return 0;
  });
}

const loginLabel = (login) => `${login.name}${login.email ? ` (${login.email})` : ''}`;

function currentOwnerLabel(root, logins = listClaudeLogins()) {
  const dir = browserOwnerConfigDir(root);
  const owner = logins.find((login) => samePath(login.configDir, dir));
  return owner ? loginLabel(owner) : dir;
}

// The owner is the Claude login whose account the browser extension is
// signed into — the one identity the bridge actually answers. Every other
// login's sessions reach the browser through a server running as it.
async function runBrowserOwner(root, requested) {
  const logins = listClaudeLogins();
  const wanted = String(requested || '').trim();

  if (!wanted) {
    console.log(`Browser owner: ${currentOwnerLabel(root, logins)}`);
    console.log('The owner is the login whose claude.ai account the browser extension is signed into.');
    console.log(`Logins: ${logins.map(loginLabel).join(', ')}`);
    console.log('Usage: bro browser owner <login|auto>');
    return 0;
  }

  // "auto" asks the bridge itself: the owner is whichever login it answers
  // with a non-empty browser list.
  if (wanted.toLowerCase() === 'auto') {
    const { claude } = ensureClaude();
    const browser = await prepareClaudeBrowser({ claudePath: claude, baseEnv: process.env, root, bridged: true });
    if (!browser) {
      console.error('Claude browser integration is disabled. Run "bro browser setup" first.');
      return 1;
    }
    for (const login of logins.filter((login) => login.authenticated)) {
      process.stderr.write(`Asking the bridge as ${loginLabel(login)}… `);
      try {
        const client = await openChromeMcp({
          claudePath: claude,
          env: { ...browser.env, CLAUDE_CONFIG_DIR: login.configDir }
        });
        try {
          const devices = await listConnectedBrowsers(client);
          if (devices.length) {
            console.error(`sees ${devices.length === 1 ? 'the browser' : `${devices.length} browsers`}.`);
            setBrowserOwner(login.local ? 'local' : login.name, root);
            console.log(`✓ Browser owner set to ${loginLabel(login)}. Every other login reaches the browser through it.`);
            return 0;
          }
          console.error('sees no browsers.');
        } finally {
          client.close();
        }
      } catch (error) {
        console.error(`bridge unreachable (${error.message}).`);
      }
    }
    console.error('✗ No login is answered by the extension. Open the browser, make sure the Claude extension is signed in to claude.ai, and retry.');
    return 1;
  }

  const login = logins.find((entry) => entry.name === wanted || entry.email === wanted);
  if (!login) {
    console.error(`Unknown login "${wanted}". Logins: ${logins.map(loginLabel).join(', ')}`);
    return 1;
  }
  if (!login.authenticated) {
    console.error(`${loginLabel(login)} is not logged in, so the bridge cannot identify as it. Log it in first.`);
    return 1;
  }
  setBrowserOwner(login.local ? 'local' : login.name, root);
  console.log(`✓ Browser owner set to ${loginLabel(login)}.`);
  console.log('  Sign the browser extension in to claude.ai as that account; every other login then shares its browser automatically.');
  return 0;
}

// Forget the pin and let the bridge pick again.
function runBrowserAuto(root) {
  setPreferredBrowser('', root);
  for (const login of listClaudeLogins()) {
    pinPairedDevice({ configDir: login.configDir, deviceId: '', browser: '' });
  }
  console.log('Browser choice cleared — sessions use whichever browser is connected.');
  return 0;
}

export async function runClaudeBrowserCommand(args = []) {
  const action = String(args[0] || 'status').toLowerCase();
  const root = claudeBrowserRoot();
  const id = sharedBridgeId(root);

  const mode = claudeBrowserMode(root);
  const backend = browserBackend(root);

  if (backend === 'mcp-chrome' && ['status', 'test', 'verify', 'reconnect'].includes(action)) {
    let state = await mcpChromeEndpointStatus();
    if (!state.connected && action !== 'status' && claudeBrowserEnabled(root)) {
      try { openMainBrowser({ browser: preferredBrowser(root) || 'Microsoft Edge' }); } catch { /* report below */ }
      const deadline = Date.now() + 10000;
      while (Date.now() < deadline && !state.connected) {
        await wait(500);
        state = await mcpChromeEndpointStatus();
      }
    }
    console.log(`Browser tools: ${claudeBrowserEnabled(root) ? 'enabled' : 'disabled'} — mcp-chrome extension bridge`);
    console.log(`Browser: ${preferredBrowser(root) || 'Microsoft Edge'}`);
    console.log(`Browsers with mcp-chrome: ${browsersWithMcpChromeExtension().join(', ') || 'none found'}`);
    console.log(`MCP endpoint: ${state.connected ? 'connected' : 'not connected'}  \x1b[2m(${MCP_CHROME_URL})\x1b[0m`);
    if (!state.connected) console.log('  Open Edge and click Connect once in the mcp-chrome extension popup.');
    if (state.connected && ['test', 'verify', 'reconnect'].includes(action)) {
      process.stderr.write('Testing the mcp-chrome tools…');
      try {
        const result = await probeMcpChrome({ callTabs: true });
        console.log('');
        console.log(`✓ MCP connected, exposed ${result.tools.length} tools, and get_windows_and_tabs answered.`);
      } catch (error) {
        console.log('');
        console.error(`✗ MCP handshake failed: ${error.message}`);
        return 1;
      }
    }
    return state.connected ? 0 : 1;
  }

  if (backend === 'mcp-chrome' && action === 'owner') {
    console.log('mcp-chrome uses the signed-in browser profile and is independent of Claude accounts; no browser owner is needed.');
    return 0;
  }

  if (backend === 'mcp-chrome' && action === 'use') {
    const wanted = resolveBrowser(args[1] || 'edge');
    if (!wanted) {
      console.error(`Unknown browser. Choices: ${CHROMIUM_BROWSERS.map((entry) => entry.id).join(', ')}`);
      return 1;
    }
    setPreferredBrowser(wanted.name, root);
    for (const login of listClaudeLogins()) writeMcpChromeProfile({ configDir: login.configDir });
    console.log(`Browser: ${wanted.name} (mcp-chrome extension).`);
    return 0;
  }

  if (action === 'status') {
    const installed = findInstalledClaudeExtension();
    console.log(`Claude browser: ${claudeBrowserEnabled(root) ? `enabled — ${mode === 'main' ? 'your own browser profile' : 'dedicated bro window'}` : 'disabled'}`);
    const chosen = preferredBrowser(root);
    const pinned = pairedDevice(root);
    console.log(`Browser: ${chosen || 'whichever one is connected'}${pinned ? `  \x1b[2m(pinned to device ${pinned.deviceId.slice(0, 8)})\x1b[0m` : ''}`);
    console.log(`Browser owner: ${currentOwnerLabel(root)}  \x1b[2m(the login the extension is signed in as — "bro browser owner" changes it)\x1b[0m`);
    console.log(`${chosen || 'Microsoft Edge'}: ${findBrowserExecutable(resolveBrowser(chosen)?.id || 'edge') || 'not found'}`);
    console.log(`Claude extension: ${installed ? `v${installed.version}` : 'not found in Edge'}`);
    console.log(`Browsers with the extension: ${browsersWithClaudeExtension().join(', ') || 'none found'}`);
    if (mode === 'main') {
      // The pipe says only that some native host is up — Claude Desktop's own
      // host holds the same name — and the browser tools never read it, so it
      // is reported as the side detail it is and the bridge is asked directly.
      const { claude } = ensureClaude();
      const { devices, error } = await probeConnectedBrowsers({ claudePath: claude, root });
      const named = devices?.length ? `${devices.length} connected` : 'nothing connected';
      console.log(`Bridge: ${error ? `unreachable — ${error}` : named}  \x1b[2m(wss://bridge.claudeusercontent.com, keyed by the owner's account)\x1b[0m`);
      console.log(`\x1b[2mNative host pipe: ${stockBridgePipeLive() ? 'held' : 'idle'} — Claude Desktop shares it, so it says nothing about the browser tools.\x1b[0m`);
      if (!error && !devices?.length) {
        console.log('  Run "bro browser reconnect" — or sign the Claude extension in to claude.ai if that does not take.');
      }
      warnAboutOtherBrowsers(root);
    } else {
      const shared = sharedUserDataDir(root);
      const ready = fs.existsSync(path.join(shared, 'Local State'));
      console.log(`Browser profile: ${ready ? 'ready' : 'not opened yet'}  ${shared}`);
      console.log(`Bridge: ${bridgePipeLive(id) ? 'connected (dedicated browser is running)' : 'not running'}  id ${id}`);
    }
    try {
      const legacy = fs.readdirSync(legacyProfilesDir(root), { withFileTypes: true }).filter((entry) => entry.isDirectory());
      if (legacy.length) {
        console.log(`Legacy per-login browsers: ${legacy.length} in ${legacyProfilesDir(root)}`);
        console.log('  Run "bro browser clean" to delete them (close their Edge windows first).');
      }
    } catch {
      /* no legacy profiles — nothing to report */
    }
    if (mode === 'main' && fs.existsSync(sharedUserDataDir(root))) {
      console.log(`Unused dedicated browser data: ${sharedUserDataDir(root)} ("bro browser clean" removes it too)`);
    }
    return 0;
  }

  if (action === 'reconnect') {
    if (!claudeBrowserEnabled(root)) {
      console.error('Claude browser integration is disabled. Run "bro browser setup" first.');
      return 1;
    }
    const { claude } = ensureClaude();
    console.log('Asking the bridge which browsers are connected…');
    const state = await ensureBrowserConnected({ claudePath: claude, root });
    if (state.connected) {
      const names = state.devices.map((device) => device.name || device.deviceId.slice(0, 8)).join(', ');
      console.log(`✓ ${state.devices.length === 1 ? 'One browser is' : `${state.devices.length} browsers are`} connected${names ? `: ${names}` : ''}.`);
      if (state.nudged) console.log('  \x1b[2mIt had dropped off and was reconnected.\x1b[0m');
      return 0;
    }
    console.error(browserDisconnectedNotice(state, root).replace(/^ {2}/gm, ''));
    return 1;
  }

  if (action === 'disable') {
    writeState({ enabled: false }, root);
    console.log('Claude browser integration disabled. Browser data was kept.');
    return 0;
  }

  if (action === 'clean') {
    const targets = [legacyProfilesDir(root), path.join(root, 'profile-pages')];
    // The dedicated profile is only a leftover while main mode is active.
    if (mode === 'main') targets.push(sharedUserDataDir(root));
    for (const target of targets) {
      try {
        fs.rmSync(target, { recursive: true, force: true });
      } catch (error) {
        console.error(`Could not delete ${target}: ${error.message}`);
        console.error('  Close every bro-opened Edge window and retry.');
        return 1;
      }
    }
    console.log(`Old bro browser data deleted.${mode === 'main' ? ' Your main Edge profile is untouched.' : ' The dedicated browser is untouched.'}`);
    return 0;
  }

  if (action === 'test') {
    return runBrowserSelfTest(root);
  }

  if (action === 'verify') {
    return runBrowserVerify(root);
  }

  if (action === 'use') {
    const wanted = args[1];
    if (wanted && ['auto', 'any', 'none'].includes(String(wanted).toLowerCase())) return runBrowserAuto(root);
    return runBrowserUse(root, wanted);
  }

  if (action === 'owner') {
    return runBrowserOwner(root, args[1]);
  }

  if (action !== 'setup' && action !== 'open') {
    console.error('Usage: bro browser <setup|open|use <browser>|owner <login|auto>|test|verify|reconnect|status|clean|disable> [--dedicated] [--no-open]');
    return 1;
  }

  const requestedClaude = args.includes('--claude-extension');
  const nextBackend = action === 'setup'
    ? ((requestedClaude || args.includes('--dedicated')) ? 'claude' : 'mcp-chrome')
    : backend;
  const nextMode = action === 'setup'
    ? (args.includes('--dedicated') ? 'dedicated' : 'main')
    : mode;

  if (nextBackend === 'mcp-chrome') {
    const asked = args.find((arg, index) => index > 0 && !arg.startsWith('-'));
    const browser = resolveBrowser(asked || 'edge');
    if (!browser) {
      console.error(`Unknown browser. Choices: ${CHROMIUM_BROWSERS.map((entry) => entry.id).join(', ')}`);
      return 1;
    }
    const installedIn = browsersWithMcpChromeExtension();
    if (!installedIn.includes(browser.name)) {
      console.error(`The official mcp-chrome extension (${MCP_CHROME_EXTENSION_ID}) was not found in ${browser.name}.`);
      console.error('  Load the unpacked GitHub release in that browser, click Connect once, then rerun setup.');
      return 1;
    }
    let host;
    try {
      host = ensureMcpChromeNativeHost();
      const fix = ensureMcpChromeMultiClientFix({ manifestPath: host.manifestPath });
      if (fix.changed) restartMcpChromeNativeHost({ manifestPath: host.manifestPath });
    } catch (error) {
      console.error(`Could not prepare mcp-chrome: ${error.message}`);
      return 1;
    }
    const profiles = listClaudeLogins();
    for (const login of profiles) writeMcpChromeProfile({ configDir: login.configDir });
    setPreferredBrowser(browser.name, root);
    if (!args.includes('--no-open')) {
      try { openMainBrowser({ browser: browser.name }); } catch (error) {
        console.error(error.message);
        return 1;
      }
    }
    let state = await mcpChromeEndpointStatus();
    const deadline = Date.now() + (args.includes('--no-open') ? 1000 : 15000);
    while (Date.now() < deadline && !state.connected) {
      await wait(500);
      state = await mcpChromeEndpointStatus();
    }
    if (!state.connected) {
      console.error(`${browser.name} is open, but mcp-chrome is not connected at ${MCP_CHROME_URL}.`);
      console.error('  Open the mcp-chrome extension popup and click Connect once, then run "bro browser setup" again.');
      return 1;
    }
    try {
      await probeMcpChrome({ callTabs: true });
    } catch (error) {
      console.error(`mcp-chrome started but get_windows_and_tabs failed: ${error.message}`);
      return 1;
    }
    writeState({
      enabled: true,
      mode: 'main',
      backend: 'mcp-chrome',
      browser: browser.name
    }, root);
    console.log(`✓ ${browser.name} is connected through mcp-chrome at ${MCP_CHROME_URL}.`);
    console.log(`  Installed the MCP config and bro-browser skill in ${profiles.length} Claude profiles. Built-in Chrome is disabled for bro launches.`);
    return 0;
  }

  if (nextMode === 'main') {
    const installedIn = browsersWithClaudeExtension();
    if (!installedIn.length) {
      console.error('The Claude in Chrome extension was not found in any browser on this machine.');
      console.error('  Install it from the store in the browser you want to use, then rerun "bro browser setup".');
      return 1;
    }
    // Naming a browser here is the same act as `bro browser use` — do it
    // before anything is started, so the right one comes up.
    const asked = args.find((arg, index) => index > 0 && !arg.startsWith('-'));
    if (asked) {
      const browser = resolveBrowser(asked);
      if (!browser) {
        console.error(`Unknown browser "${asked}". Choices: ${CHROMIUM_BROWSERS.map((entry) => entry.id).join(', ')}`);
        return 1;
      }
      setPreferredBrowser(browser.name, root);
    }
    const chosen = preferredBrowser(root) || (installedIn.includes('Microsoft Edge') ? 'Microsoft Edge' : installedIn[0]);
    // "open" re-enables too — asking for the browser is opting into it.
    writeState({ enabled: true, mode: 'main', backend: 'claude' }, root);
    if (stockBridgePipeLive()) {
      console.log(`Connected: ${chosen} is running and its Claude extension is serving the bridge.`);
    } else if (action === 'setup' && args.includes('--no-open')) {
      console.log(`Enabled. ${chosen} will be started automatically with the next bro session.`);
    } else {
      try {
        openMainBrowser({ browser: chosen });
      } catch (error) {
        console.error(error.message);
        return 1;
      }
      process.stderr.write(`Starting ${chosen} and waiting for the Claude extension to connect…`);
      const deadline = Date.now() + 15000;
      while (Date.now() < deadline && !stockBridgePipeLive()) await wait(500);
      console.log('');
      if (stockBridgePipeLive()) {
        console.log(`Connected: the Claude extension in ${chosen} is serving the bridge.`);
      } else {
        console.log(`${chosen} started, but the Claude extension has not connected yet.`);
        console.log('  Make sure the extension is enabled, then check "bro browser status".');
      }
    }
    // Several browsers with the extension is the one setup that still needs a
    // decision; making it now costs one probe and settles it for good.
    if (installedIn.length > 1 && !pairedDevice(root) && stockBridgePipeLive()) {
      await runBrowserUse(root, chosen);
    } else {
      warnAboutOtherBrowsers(root);
    }
    if (fs.existsSync(sharedUserDataDir(root))) {
      console.log('You can close any dedicated bro browser window; "bro browser clean" removes its data.');
    }
    console.log('Every model bro launches now shares that browser — Claude, GLM, Kimi, the account pool, several at once.');
    return 0;
  }

  const { claude } = ensureClaude();
  const infrastructure = ensureClaudeBrowserInfrastructure({ root, claudePath: claude });
  writeState({ enabled: true, mode: 'dedicated' }, root);

  if (action === 'setup' && args.includes('--no-open')) {
    console.log('Dedicated Claude browser enabled. It will open automatically with the next bro session.');
    return 0;
  }

  let opened;
  try {
    opened = await openClaudeBrowser({
      claudePath: claude,
      root,
      signIn: action === 'setup',
      // An explicit "open" always brings up a window; setup silently reuses a
      // browser that is already connected.
      reuseIfRunning: action !== 'open',
      infrastructure
    });
  } catch (error) {
    console.error(error.message);
    return 1;
  }
  if (opened.reused) {
    console.log('The dedicated Claude browser is already running and connected.');
  } else {
    if (opened.adopted) console.log(`Adopted your most recent per-login browser ("${opened.adopted.name}") as the dedicated one — its claude.ai sign-in carries over.`);
    if (action === 'setup') console.log('Edge is opening — sign in to claude.ai once in this window if it asks.');
    else console.log('Edge is opening the dedicated Claude browser.');
  }
  console.log('Every Claude profile\'s sessions now share this one browser window automatically.');
  return 0;
}
