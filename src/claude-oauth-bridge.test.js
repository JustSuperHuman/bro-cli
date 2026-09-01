import { expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { claudeCodeLoginStatus, claudeOAuthBridgeProvider } from './claude-oauth-bridge.js';

test('Claude login detection reports metadata without exposing credentials', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-claude-login-test-'));
  try {
    fs.writeFileSync(path.join(root, '.credentials.json'), JSON.stringify({
      claudeAiOauth: {
        accessToken: 'sk-ant-oat-secret',
        refreshToken: 'refresh-secret',
        expiresAt: 123456789,
        subscriptionType: 'max'
      }
    }));
    const status = claudeCodeLoginStatus({ configDir: root });
    expect(status).toEqual({
      configDir: root,
      authenticated: true,
      subscriptionType: 'max',
      expiresAt: 123456789
    });
    expect(JSON.stringify(status)).not.toContain('sk-ant-oat-secret');
    expect(JSON.stringify(status)).not.toContain('refresh-secret');
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('the Claude OAuth bridge keeps the Anthropic catalog route and live models', () => {
  const provider = {
    id: 'anthropic',
    name: 'Claude (Anthropic)',
    mode: 'native',
    models: [{ name: 'Default' }, { id: 'claude-sonnet-4-6', name: 'Claude Sonnet 4.6' }]
  };
  const bridged = claudeOAuthBridgeProvider(provider, {
    baseUrl: 'http://127.0.0.1:45678',
    models: [
      { id: 'claude-opus-4-8', name: 'Claude Opus 4.8' },
      { id: 'claude-sonnet-5', name: 'Claude Sonnet 5' }
    ]
  });

  expect(bridged).toMatchObject({
    id: 'anthropic',
    mode: 'anthropic',
    baseUrl: 'http://127.0.0.1:45678',
    dshRoute: 'anthropic',
    noKey: false
  });
  expect(bridged.models.map((model) => model.id)).toEqual([
    'claude-opus-4-8',
    'claude-sonnet-5'
  ]);
});
