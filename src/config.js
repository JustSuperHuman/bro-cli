import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { stripHash } from './strip.js';

export const BRO_DIR = path.join(os.homedir(), '.bro');
export const CONFIG_PATH = path.join(BRO_DIR, 'config.json');

// Written verbatim the first time bro runs. The '#'-prefixed fields are example /
// test data that the loader ignores — they double as inline documentation.
const DEFAULT_CONFIG = {
  '#': 'bro config. Anything whose key/id/name starts with # is ignored.',
  '#docs': 'https://justgains.com',
  // How Claude Code starts: 'auto' (intelligent auto-mode, the default),
  // 'manual' (prompt for everything), or 'bypass' (--dangerously-skip-permissions).
  permissionMode: 'auto',
  '#dangerouslySkipPermissions': 'deprecated — use permissionMode instead (true → bypass, false → manual)',
  keys: {
    '#sakana': 'fish_xxx   (remove the # and rename the key to "sakana" to use it)',
    '#openrouter': 'sk-or-xxx',
    '#openai': 'sk-xxx'
  },
  providers: [
    {
      '#': 'Example custom provider — drop the # from id and name to enable it.',
      id: '#my-local',
      name: '#My Local Model',
      mode: 'openai',
      baseUrl: 'http://localhost:1234/v1/chat/completions',
      noKey: true,
      models: [
        { id: 'my-model', name: 'My Model' },
        { '#id': 'another-model-thats-ignored' }
      ]
    }
  ]
};

export function loadRawConfig() {
  try {
    return JSON.parse(fs.readFileSync(CONFIG_PATH, 'utf8'));
  } catch {
    return null;
  }
}

// Cleaned config (with all '#' test data stripped) for the app to use.
export function loadConfig() {
  return stripHash(loadRawConfig() ?? DEFAULT_CONFIG);
}

export function ensureDefaultConfig() {
  if (fs.existsSync(CONFIG_PATH)) return false;
  fs.mkdirSync(BRO_DIR, { recursive: true });
  fs.writeFileSync(CONFIG_PATH, JSON.stringify(DEFAULT_CONFIG, null, 2));
  return true;
}

// Resolve the Claude Code permission posture from config:
//   'auto'   — Claude's intelligent auto-mode (--permission-mode auto)  [default]
//   'manual' — prompt for everything (Claude's normal mode)
//   'bypass' — skip every permission check (--dangerously-skip-permissions)
// `permissionMode` is the source of truth; the legacy `dangerouslySkipPermissions`
// boolean is honored only when `permissionMode` is unset, so existing configs keep
// working after upgrading.
export function configPermissionMode(config = {}) {
  const m = config.permissionMode;
  if (m === 'auto' || m === 'manual' || m === 'bypass') return m;
  if (config.dangerouslySkipPermissions === true) return 'bypass';
  if (config.dangerouslySkipPermissions === false) return 'manual';
  return 'auto';
}

// Persist a key without disturbing the user's '#' notes/examples.
export function setKey(providerId, key) {
  const raw = loadRawConfig() ?? structuredClone(DEFAULT_CONFIG);
  raw.keys = raw.keys || {};
  raw.keys[providerId] = key;
  fs.mkdirSync(BRO_DIR, { recursive: true });
  fs.writeFileSync(CONFIG_PATH, JSON.stringify(raw, null, 2));
}
