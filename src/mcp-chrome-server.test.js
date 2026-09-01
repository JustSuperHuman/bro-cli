import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  MCP_CHROME_EXTENSION_ID,
  MCP_CHROME_SERVER_NAME,
  MCP_CHROME_URL,
  mcpChromeConfig,
  validateNativeHostManifest,
  writeMcpChromeProfile
} from './mcp-chrome-server.js';

test('mcp-chrome config exactly matches the extension-provided Streamable HTTP endpoint', () => {
  expect(mcpChromeConfig()).toEqual({
    mcpServers: {
      [MCP_CHROME_SERVER_NAME]: {
        type: 'streamable-http',
        url: MCP_CHROME_URL
      }
    }
  });
});

test('profile wiring is stable and removes only obsolete bro DevTools files', () => {
  const configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-mcp-chrome-profile-'));
  try {
    const oldRoot = path.join(configDir, 'bro-browser');
    fs.mkdirSync(oldRoot, { recursive: true });
    fs.writeFileSync(path.join(oldRoot, 'devtools-mcp.json'), 'old');
    fs.writeFileSync(path.join(oldRoot, 'devtools-prompt.md'), 'old');
    const first = writeMcpChromeProfile({ configDir });
    const before = fs.statSync(first.configPath).mtimeMs;
    const second = writeMcpChromeProfile({ configDir });
    expect(second).toEqual(first);
    expect(fs.statSync(second.configPath).mtimeMs).toBe(before);
    expect(fs.existsSync(path.join(oldRoot, 'devtools-mcp.json'))).toBe(false);
    expect(fs.existsSync(path.join(oldRoot, 'devtools-prompt.md'))).toBe(false);
    expect(fs.readFileSync(first.skillPath, 'utf8'))
      .toContain(`mcp__${MCP_CHROME_SERVER_NAME}__get_windows_and_tabs`);
  } finally {
    fs.rmSync(configDir, { recursive: true, force: true });
  }
});

test('native host validation requires the reviewed host, wrapper, and extension origin', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-mcp-chrome-host-'));
  try {
    const wrapper = path.join(root, 'run_host.bat');
    const manifestPath = path.join(root, 'host.json');
    fs.writeFileSync(wrapper, '@echo off\n');
    fs.writeFileSync(manifestPath, JSON.stringify({
      name: 'com.chromemcp.nativehost',
      description: 'test',
      path: wrapper,
      type: 'stdio',
      allowed_origins: [`chrome-extension://${MCP_CHROME_EXTENSION_ID}/`]
    }));
    expect(validateNativeHostManifest(manifestPath).manifest.path).toBe(wrapper);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
