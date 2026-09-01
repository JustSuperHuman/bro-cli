// Claude Code's browser bridge, reachable by any model.
//
// `claude --claude-in-chrome-mcp` is a plain stdio MCP server sitting in front
// of the Claude extension's native host. The bridge behind it is
// account-scoped: the server identifies itself with the claude.ai account of
// whatever CLAUDE_CONFIG_DIR profile it reads, and the extension only serves
// the account it is signed into — any other account sees zero connected
// browsers over the same live pipe (verified empirically against Claude
// Code's bridge handshake, which sends the profile's oauthAccount.accountUuid
// and OAuth token). bro therefore spawns the server AS the browser-owner
// login (the one whose account the extension is signed into) via the config's
// env, and every session — another pool account, GLM through Z.ai, the Codex
// bridge — reaches the browser through it. Claude Code's *own* wiring is
// doubly out of reach for those sessions: it is gated on claude.ai account
// auth (ANTHROPIC_AUTH_TOKEN switches it off without a word) and, even when
// that gate passes, it identifies as the session's own account and gets an
// empty browser list. Configuring the same server explicitly with the owner's
// identity walks around both gates instead of defeating them: it is the exact
// server Claude Code configures for itself, running as the login the
// extension actually answers.
//
// This module is both halves of that arrangement — the server description bro
// hands to a session, and a small client bro drives itself, so questions like
// "which browsers are connected, and which one is Edge?" cost milliseconds
// rather than a model turn.

import { spawn } from 'node:child_process';

export const CHROME_MCP_FLAG = '--claude-in-chrome-mcp';

// Claude Code reserves its own "claude-in-chrome" for the built-in wiring and
// drops any --mcp-config entry using that name (with a warning nothing
// surfaces). bro's copy therefore answers to "chrome", which is also what the
// tools end up called: mcp__chrome__navigate and friends.
export const CHROME_MCP_SERVER_NAME = 'chrome';

// Claude Code's own values for the extension's per-site permission prompts.
// "skip_all_permission_checks" is what it passes when the session is already
// running with permissions bypassed, so bro mirrors that rather than inventing
// a policy of its own.
export const CHROME_PERMISSION_MODES = ['ask', 'skip_all_permission_checks', 'follow_a_plan'];

// New-tab pages are the cheapest possible browser fingerprint: every Chromium
// fork serves its own scheme there, with no navigation and no network. Only
// the fork schemes are conclusive — "chrome://" is the scheme the extension
// APIs use everywhere, and a fork rewrites it to its own a moment after the
// tab appears, so seeing it proves nothing until it has stopped changing.
const FORK_SCHEMES = [
  [/^edge:/i, 'Microsoft Edge'],
  [/^brave:/i, 'Brave'],
  [/^vivaldi:/i, 'Vivaldi'],
  [/^opera:/i, 'Opera'],
  [/^arc:/i, 'Arc']
];

export function forkFromNewTabUrl(url) {
  return FORK_SCHEMES.find(([pattern]) => pattern.test(String(url || '')))?.[1] || null;
}

export function browserFromNewTabUrl(url) {
  return forkFromNewTabUrl(url) || (/^chrome:/i.test(String(url || '')) ? 'Google Chrome' : null);
}

const isWebPage = (url) => /^https?:/i.test(String(url || ''));

// A browser whose new tab was replaced (by an extension, or a homepage) lands
// on a normal page instead, where reading the user agent still works.
export function browserFromUserAgent(ua) {
  if (/\bEdg(?:e|A|iOS)?\//.test(ua)) return 'Microsoft Edge';
  if (/\bOPR\//.test(ua)) return 'Opera';
  if (/\bVivaldi\//.test(ua)) return 'Vivaldi';
  if (/\bBrave\//.test(ua)) return 'Brave';
  if (/\bChrome\//.test(ua)) return 'Google Chrome (or another Chromium browser)';
  return null;
}

export function chromeMcpServer({ claudePath, skipPermissions = false, ownerConfigDir = '' }) {
  const env = {
    // The bridge serves the account this profile is signed in as, so the
    // server always runs as the browser-owner login regardless of which
    // session spawned it. CLAUDE_CODE_ENABLE_CFC neutralizes the "false" a
    // foreign-login session carries to keep its own built-in wiring quiet.
    ...(ownerConfigDir ? { CLAUDE_CONFIG_DIR: ownerConfigDir, CLAUDE_CODE_ENABLE_CFC: 'true' } : {}),
    ...(skipPermissions ? { CLAUDE_CHROME_PERMISSION_MODE: 'skip_all_permission_checks' } : {})
  };
  return {
    type: 'stdio',
    command: claudePath,
    args: [CHROME_MCP_FLAG],
    ...(Object.keys(env).length ? { env } : {})
  };
}

export function chromeMcpConfig({ claudePath, skipPermissions = false, ownerConfigDir = '' }) {
  return { mcpServers: { [CHROME_MCP_SERVER_NAME]: chromeMcpServer({ claudePath, skipPermissions, ownerConfigDir }) } };
}

// --- a minimal MCP stdio client -------------------------------------------

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

class ChromeMcpError extends Error {}

// Newline-delimited JSON-RPC over the server's stdio. Deliberately tiny: bro
// calls a handful of tools and needs no notifications, sampling or roots.
class ChromeMcpClient {
  #child;
  #pending = new Map();
  #buffer = '';
  #nextId = 1;
  #timeoutMs;
  #exit = null;

  constructor(child, timeoutMs) {
    this.#child = child;
    this.#timeoutMs = timeoutMs;
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => this.#read(chunk));
    // The server logs bridge diagnostics to stderr; they are only interesting
    // when something fails, and the caller decides that.
    child.stderr.resume();
    const fail = (reason) => {
      this.#exit = reason;
      for (const [, entry] of this.#pending) entry.reject(new ChromeMcpError(reason));
      this.#pending.clear();
    };
    child.on('exit', (code) => fail(`the browser bridge exited (code ${code ?? 0})`));
    child.on('error', (error) => fail(error.message));
  }

  #read(chunk) {
    this.#buffer += chunk;
    let newline;
    while ((newline = this.#buffer.indexOf('\n')) !== -1) {
      const line = this.#buffer.slice(0, newline).trim();
      this.#buffer = this.#buffer.slice(newline + 1);
      if (!line) continue;
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        continue; // not a JSON-RPC frame — the server is free to be chatty
      }
      const entry = this.#pending.get(message.id);
      if (!entry) continue;
      this.#pending.delete(message.id);
      clearTimeout(entry.timer);
      if (message.error) entry.reject(new ChromeMcpError(message.error.message || 'browser bridge error'));
      else entry.resolve(message.result);
    }
  }

  #send(message) {
    this.#child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  request(method, params, { timeoutMs = this.#timeoutMs } = {}) {
    if (this.#exit) return Promise.reject(new ChromeMcpError(this.#exit));
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new ChromeMcpError(`${method} timed out after ${Math.round(timeoutMs / 1000)}s`));
      }, timeoutMs);
      this.#pending.set(id, { resolve, reject, timer });
      this.#send({ jsonrpc: '2.0', id, method, params });
    });
  }

  notify(method, params) {
    if (!this.#exit) this.#send({ jsonrpc: '2.0', method, params });
  }

  // Tool results arrive as content blocks; every tool bro calls answers in
  // text, and the first block is the machine-readable one.
  async callTool(name, args = {}, options) {
    const result = await this.request('tools/call', { name, arguments: args }, options);
    const text = (result?.content || [])
      .filter((block) => block?.type === 'text')
      .map((block) => block.text)
      .join('\n');
    if (result?.isError) throw new ChromeMcpError(text || `${name} failed`);
    return text;
  }

  // Several tools answer with a JSON object followed by a prose restatement of
  // it (and sometimes a system reminder); only the first line is data.
  async callToolJson(name, args = {}, options) {
    const text = await this.callTool(name, args, options);
    const [first] = text.split('\n');
    try {
      return JSON.parse(first);
    } catch {
      throw new ChromeMcpError(`${name} did not answer with JSON: ${text.slice(0, 120)}`);
    }
  }

  close() {
    for (const [, entry] of this.#pending) clearTimeout(entry.timer);
    this.#pending.clear();
    try {
      this.#child.kill();
    } catch {
      /* already gone */
    }
  }
}

// Start the bridge and complete the MCP handshake. `env` is passed through
// untouched: a caller that wants the bridge to read a particular Claude
// profile's pairing sets CLAUDE_CONFIG_DIR in it.
export async function openChromeMcp({
  claudePath,
  env = process.env,
  timeoutMs = 30000,
  spawnProcess = spawn
} = {}) {
  if (!claudePath) throw new ChromeMcpError('Claude Code executable path is required to reach the browser.');
  const child = spawnProcess(claudePath, [CHROME_MCP_FLAG], {
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true
  });
  const client = new ChromeMcpClient(child, timeoutMs);
  try {
    await client.request('initialize', {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'bro', version: '1' }
    });
    client.notify('notifications/initialized', {});
    return client;
  } catch (error) {
    client.close();
    throw error;
  }
}

export function listConnectedBrowsers(client) {
  return client.callToolJson('list_connected_browsers');
}

// Identify the selected browser from one tab. Its new-tab scheme names it
// outright; a replaced new-tab page falls through to the user agent, which
// needs a real page but is just as conclusive. Only tabs this opened are
// closed again — including when identification fails partway.
export async function identifySelectedBrowser(client, { settleMs = 300, attempts = 6 } = {}) {
  // Before any tab group exists both tools answer in prose rather than JSON:
  // tabs_context_mcp reports no group, and tabs_create_mcp refuses until
  // createIfEmpty has made one. So the group is established first, and what
  // that call brings into being is counted as ours to clean up.
  const tabsOf = async (createIfEmpty = false) => {
    try {
      return (await client.callToolJson('tabs_context_mcp', { createIfEmpty })).availableTabs || [];
    } catch {
      return [];
    }
  };

  const before = await tabsOf();
  const known = new Set(before.map((tab) => tab.tabId));
  let ours = (await tabsOf(true)).filter((tab) => !known.has(tab.tabId));

  if (!ours.length) {
    // The group was already open with the user's own tabs in it, so take a
    // fresh one rather than reading a page they are working in.
    await client.callTool('tabs_create_mcp').catch(() => {});
    ours = (await tabsOf()).filter((tab) => !known.has(tab.tabId));
  }

  const tab = ours[0];
  if (!tab) return { browser: null, reason: 'the browser did not report the tab that was opened' };

  try {
    // Watch the tab settle. A fork scheme is conclusive the instant it shows
    // up, so Edge is never mistaken for Chrome on the way there; a URL that
    // only ever says "chrome://" has genuinely settled on Chrome.
    let url = tab.url || '';
    for (let attempt = 0; attempt < attempts; attempt++) {
      const fork = forkFromNewTabUrl(url);
      if (fork) return { browser: fork, evidence: url };
      if (isWebPage(url)) break;
      await wait(settleMs);
      url = (await tabsOf()).find((entry) => entry.tabId === tab.tabId)?.url || url;
    }
    if (/^chrome:/i.test(url)) return { browser: 'Google Chrome', evidence: url };

    // A replaced new-tab page lands on a real page, where the URL says nothing
    // but the user agent still does.
    if (isWebPage(url)) {
      const ua = await client.callTool('javascript_tool', {
        action: 'javascript_exec',
        text: 'navigator.userAgent',
        tabId: tab.tabId
      });
      const byAgent = browserFromUserAgent(ua);
      if (byAgent) return { browser: byAgent, evidence: ua.trim().slice(0, 160) };
      return { browser: null, reason: `unrecognized user agent (${ua.trim().slice(0, 80)})` };
    }
    return { browser: null, reason: `the tab never reported a page (${url || 'no url'})` };
  } catch (error) {
    return { browser: null, reason: error.message };
  } finally {
    // Best effort: a browser can refuse to close its own last new-tab page
    // ("Cannot remove NTP tab"), which leaves an empty tab the next probe
    // simply reuses. Never worth failing an identification over.
    for (const opened of ours) {
      await client.callTool('tabs_close_mcp', { tabId: opened.tabId }).catch(() => {});
    }
  }
}

// Every connected browser, each named. One browser needs no selection at all —
// the bridge auto-connects to it — so it is identified in place.
export async function describeConnectedBrowsers(client) {
  const devices = await listConnectedBrowsers(client);
  const described = [];
  for (const device of devices) {
    if (devices.length > 1) await client.callTool('select_browser', { deviceId: device.deviceId });
    const { browser, evidence, reason } = await identifySelectedBrowser(client);
    described.push({ ...device, browser, evidence, reason });
  }
  return described;
}
