import { expect, test } from 'bun:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import {
  CHROME_MCP_FLAG,
  browserFromNewTabUrl,
  browserFromUserAgent,
  chromeMcpServer,
  describeConnectedBrowsers,
  identifySelectedBrowser,
  listConnectedBrowsers,
  openChromeMcp
} from './chrome-mcp.js';

// A stand-in for `claude --claude-in-chrome-mcp`: it answers whatever the
// test scripts for each tool, and records what it was asked, so the wire
// protocol is exercised for real without a browser anywhere near it.
function fakeBridge({ tools = {}, onRequest } = {}) {
  const calls = [];
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = new PassThrough();
  child.killed = false;
  child.kill = () => { child.killed = true; };

  const reply = (message) => child.stdout.write(`${JSON.stringify(message)}\n`);
  let buffer = '';
  child.stdin.on('data', (chunk) => {
    buffer += chunk.toString();
    let newline;
    while ((newline = buffer.indexOf('\n')) !== -1) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (!line.trim()) continue;
      const request = JSON.parse(line);
      calls.push(request);
      if (request.id == null) continue; // a notification
      if (onRequest?.(request, reply)) continue;
      if (request.method === 'initialize') {
        reply({ jsonrpc: '2.0', id: request.id, result: { serverInfo: { name: 'Claude in Chrome', version: '1.0.0' } } });
        continue;
      }
      if (request.method === 'tools/call') {
        const answer = tools[request.params.name];
        const value = typeof answer === 'function' ? answer(request.params.arguments, calls) : answer;
        reply({
          jsonrpc: '2.0',
          id: request.id,
          result: { content: [{ type: 'text', text: String(value?.text ?? value ?? '') }], isError: Boolean(value?.isError) }
        });
        continue;
      }
      reply({ jsonrpc: '2.0', id: request.id, error: { code: -32601, message: `no ${request.method}` } });
    }
  });

  const spawnProcess = (command, args, options) => {
    calls.push({ spawned: { command, args, options } });
    return child;
  };
  return { child, calls, spawnProcess, toolCalls: () => calls.filter((c) => c.method === 'tools/call').map((c) => c.params) };
}

const tabs = (list) => ({ text: `${JSON.stringify({ availableTabs: list, tabGroupId: 7 })}\n\nTab Context:\n- prose restatement` });

test('the bridge is started as Claude Code starts it, and the handshake completes', async () => {
  const bridge = fakeBridge();
  const client = await openChromeMcp({ claudePath: 'C:\\claude.exe', spawnProcess: bridge.spawnProcess });
  try {
    const spawned = bridge.calls[0].spawned;
    expect(spawned.command).toBe('C:\\claude.exe');
    expect(spawned.args).toEqual([CHROME_MCP_FLAG]);
    expect(bridge.calls[1].method).toBe('initialize');
    expect(bridge.calls[2].method).toBe('notifications/initialized');
  } finally {
    client.close();
  }
  expect(bridge.child.killed).toBe(true);
});

test('connected browsers are read from the first line, past the prose the tool adds', async () => {
  const devices = [{ deviceId: 'device-a', name: 'Browser 1', osPlatform: 'Windows', isLocal: true }];
  const bridge = fakeBridge({
    tools: { list_connected_browsers: { text: `${JSON.stringify(devices)}\n\nThat is 1 browser.` } }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    expect(await listConnectedBrowsers(client)).toEqual(devices);
  } finally {
    client.close();
  }
});

test('a failing tool call surfaces the bridge’s own message', async () => {
  const bridge = fakeBridge({
    tools: { list_connected_browsers: { text: 'Extension is not connected', isError: true } }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    expect(listConnectedBrowsers(client)).rejects.toThrow(/not connected/);
  } finally {
    client.close();
  }
});

test('a browser names itself through its new tab page, and the tab is cleaned up', async () => {
  let created = false;
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: () => (created
        ? tabs([{ tabId: 1, url: 'edge://newtab/' }, { tabId: 2, url: 'edge://newtab/' }])
        : tabs([{ tabId: 1, url: 'edge://newtab/' }])),
      tabs_create_mcp: () => { created = true; return 'Created new tab. Tab ID: 2'; },
      tabs_close_mcp: 'Closed tab 2.'
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    const identified = await identifySelectedBrowser(client, { settleMs: 0 });
    expect(identified.browser).toBe('Microsoft Edge');
    expect(bridge.toolCalls().at(-1)).toEqual({ name: 'tabs_close_mcp', arguments: { tabId: 2 } });
  } finally {
    client.close();
  }
});

// Before any tab group exists both tools answer in prose rather than JSON:
// the context tool reports no group, and creating a tab is refused until
// createIfEmpty has made one. Identification has to walk through that.
test('a browser with no tab group yet is identified, and its group is cleaned up', async () => {
  let group = null;
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: (args) => {
        if (!group) {
          if (!args.createIfEmpty) return { text: 'No MCP tab groups found. Use createIfEmpty: true to create one.' };
          group = [{ tabId: 3, url: 'edge://newtab/' }];
        }
        return tabs(group);
      },
      tabs_create_mcp: () => (group
        ? 'Created new tab.'
        : { text: 'No MCP tab group exists. Use tabs_context_mcp with createIfEmpty: true first.', isError: true }),
      tabs_close_mcp: (args) => {
        group = group.filter((tab) => tab.tabId !== args.tabId);
        return `Closed tab ${args.tabId}.`;
      }
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    expect((await identifySelectedBrowser(client, { settleMs: 0 })).browser).toBe('Microsoft Edge');
    // The group this opened is the group it closes: nothing is left behind.
    expect(group).toEqual([]);
    expect(bridge.toolCalls().some((call) => call.name === 'tabs_create_mcp')).toBe(false);
  } finally {
    client.close();
  }
});

// "chrome://newtab/" is what the extension APIs report before a fork rewrites
// it to its own scheme, so reading the URL once would call Edge "Chrome".
test('a fork that first reports chrome:// is not mistaken for Chrome', async () => {
  // The tab appears with no URL, is briefly reported as chrome://, and only
  // then settles on the fork's own scheme.
  const seen = ['', 'chrome://newtab/', 'chrome://newtab/', 'edge://newtab/'];
  let group = null;
  let read = 0;
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: (args) => {
        if (!group && args.createIfEmpty) group = 5;
        return tabs(group ? [{ tabId: group, url: seen[Math.min(read++, seen.length - 1)] }] : []);
      },
      tabs_create_mcp: 'Created new tab.',
      tabs_close_mcp: () => { group = null; return 'Closed tab 5.'; }
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    const identified = await identifySelectedBrowser(client, { settleMs: 0 });
    expect(identified.browser).toBe('Microsoft Edge');
    expect(identified.evidence).toBe('edge://newtab/');
  } finally {
    client.close();
  }
});

test('a URL that only ever says chrome:// really is Chrome', async () => {
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: (args) => tabs(args.createIfEmpty ? [{ tabId: 6, url: 'chrome://newtab/' }] : []),
      tabs_create_mcp: 'Created new tab.',
      tabs_close_mcp: 'Closed tab 6.'
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    expect((await identifySelectedBrowser(client, { settleMs: 0, attempts: 3 })).browser).toBe('Google Chrome');
  } finally {
    client.close();
  }
});

// A browser whose new tab was replaced lands on a normal page, where the URL
// says nothing but the user agent still does.
test('a replaced new tab page falls back to the user agent', async () => {
  let created = false;
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: () => (created ? tabs([{ tabId: 9, url: 'https://start.example' }]) : tabs([])),
      tabs_create_mcp: () => { created = true; return 'Created new tab. Tab ID: 9'; },
      javascript_tool: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/151.0.0.0 Safari/537.36 OPR/117.0.0.0',
      tabs_close_mcp: 'Closed tab 9.'
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    expect((await identifySelectedBrowser(client, { settleMs: 0 })).browser).toBe('Opera');
  } finally {
    client.close();
  }
});

test('a tab that cannot be identified still gets closed', async () => {
  let created = false;
  const bridge = fakeBridge({
    tools: {
      tabs_context_mcp: () => (created ? tabs([{ tabId: 4, url: 'https://start.example' }]) : tabs([])),
      tabs_create_mcp: () => { created = true; return 'Created new tab. Tab ID: 4'; },
      javascript_tool: { text: 'Failed to execute JavaScript', isError: true },
      tabs_close_mcp: 'Closed tab 4.'
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    const identified = await identifySelectedBrowser(client, { settleMs: 0 });
    expect(identified.browser).toBe(null);
    expect(identified.reason).toMatch(/Failed to execute/);
    expect(bridge.toolCalls().at(-1)).toEqual({ name: 'tabs_close_mcp', arguments: { tabId: 4 } });
  } finally {
    client.close();
  }
});

// One connected browser is auto-selected by the bridge, so selecting it would
// be noise; several have to be visited one at a time.
test('several connected browsers are each selected before being identified', async () => {
  const devices = [
    { deviceId: 'device-a', name: 'Browser 1', isLocal: true },
    { deviceId: 'device-b', name: 'Browser 2', isLocal: true }
  ];
  // Each browser keeps its own tabs, so identifying the second one starts from
  // its own empty group rather than inheriting the first one's.
  const worlds = {
    'device-a': { url: 'edge://newtab/', open: [] },
    'device-b': { url: 'chrome://newtab/', open: [] }
  };
  let selected = 'device-a';
  let nextTabId = 1;
  const bridge = fakeBridge({
    tools: {
      list_connected_browsers: JSON.stringify(devices),
      select_browser: (args) => { selected = args.deviceId; return 'Selected.'; },
      tabs_context_mcp: () => tabs(worlds[selected].open),
      tabs_create_mcp: () => {
        const tabId = nextTabId++;
        worlds[selected].open.push({ tabId, url: worlds[selected].url });
        return `Created new tab. Tab ID: ${tabId}`;
      },
      tabs_close_mcp: (args) => {
        worlds[selected].open = worlds[selected].open.filter((tab) => tab.tabId !== args.tabId);
        return `Closed tab ${args.tabId}.`;
      }
    }
  });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  try {
    const described = await describeConnectedBrowsers(client);
    expect(described.map((entry) => entry.browser)).toEqual(['Microsoft Edge', 'Google Chrome']);
    expect(bridge.toolCalls().filter((call) => call.name === 'select_browser').map((call) => call.arguments.deviceId))
      .toEqual(['device-a', 'device-b']);
  } finally {
    client.close();
  }
});

test('a bridge that dies mid-question fails the caller rather than hanging', async () => {
  const bridge = fakeBridge({ onRequest: (request) => request.method === 'tools/call' });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess });
  const pending = listConnectedBrowsers(client);
  bridge.child.emit('exit', 1);
  expect(pending).rejects.toThrow(/exited/);
  client.close();
});

test('a request that outlives its timeout gives up with the method named', async () => {
  const bridge = fakeBridge({ onRequest: (request) => request.method === 'tools/call' });
  const client = await openChromeMcp({ claudePath: 'claude', spawnProcess: bridge.spawnProcess, timeoutMs: 30 });
  try {
    expect(client.callTool('navigate')).rejects.toThrow(/tools\/call timed out/);
  } finally {
    client.close();
  }
});

test('the server description matches the one Claude Code configures for itself', () => {
  expect(chromeMcpServer({ claudePath: 'claude.exe' })).toEqual({
    type: 'stdio',
    command: 'claude.exe',
    args: [CHROME_MCP_FLAG]
  });
  expect(chromeMcpServer({ claudePath: 'claude.exe', skipPermissions: true }).env)
    .toEqual({ CLAUDE_CHROME_PERMISSION_MODE: 'skip_all_permission_checks' });
});

test('browser fingerprints cover both the tab scheme and the user agent', () => {
  expect(browserFromNewTabUrl('vivaldi://startpage')).toBe('Vivaldi');
  expect(browserFromNewTabUrl('about:blank')).toBe(null);
  expect(browserFromUserAgent('Mozilla/5.0 Chrome/151.0.0.0 Edg/151.0.0.0')).toBe('Microsoft Edge');
  expect(browserFromUserAgent('Mozilla/5.0 Firefox/143.0')).toBe(null);
});
