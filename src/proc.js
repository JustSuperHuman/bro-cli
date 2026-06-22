import { spawn, spawnSync, execSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const isWin = process.platform === 'win32';

// Cross-platform `which`. On Windows it honours PATHEXT (.exe/.cmd/.bat...).
export function which(name, extraDirs = []) {
  const exts = isWin ? (process.env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';') : [''];
  const dirs = [...extraDirs, ...(process.env.PATH || '').split(path.delimiter)];
  for (const d of dirs) {
    if (!d) continue;
    for (const e of exts) {
      const p = path.join(d, name + e);
      try {
        if (fs.statSync(p).isFile()) return p;
      } catch {
        /* keep looking */
      }
    }
  }
  return null;
}

// Where bun / npm drop globally-installed bin shims (so we can find `ccr`
// even when that directory isn't on PATH — common on Windows).
export function globalBinDirs() {
  const dirs = [path.join(os.homedir(), '.bun', 'bin')];
  if (process.env.BUN_INSTALL) dirs.push(path.join(process.env.BUN_INSTALL, 'bin'));
  if (process.env.APPDATA) dirs.push(path.join(process.env.APPDATA, 'npm'));
  try {
    const prefix = execSync('npm config get prefix', { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
    if (prefix) dirs.push(prefix, path.join(prefix, 'bin'));
  } catch {
    /* npm not installed */
  }
  return [...new Set(dirs)];
}

function winQuote(a) {
  return /[\s"]/.test(a) ? '"' + a.replace(/"/g, '\\"') + '"' : a;
}

// Spawn inheriting stdio. Handles Windows .cmd/.bat shims (npm), which can't be
// spawned directly. Resolves with the child's exit code.
export function runInherit(file, args, env = process.env) {
  return new Promise((resolve) => {
    const ext = path.extname(file).toLowerCase();
    let child;
    if (isWin && (ext === '.cmd' || ext === '.bat')) {
      const line = [file, ...args].map(winQuote).join(' ');
      child = spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', line], {
        stdio: 'inherit',
        env,
        windowsVerbatimArguments: true
      });
    } else {
      child = spawn(file, args, { stdio: 'inherit', env });
    }
    child.on('exit', (code) => resolve(code ?? 0));
    child.on('error', (err) => {
      console.error(err.message);
      resolve(1);
    });
  });
}

// Make sure the Anthropic<->OpenAI proxy (claude-code-router / `ccr`) is present,
// installing it with bun or npm if needed. Returns { ccr, dirs }.
export function ensureProxy() {
  let dirs = globalBinDirs();
  let ccr = which('ccr', dirs);
  if (ccr) return { ccr, dirs };

  const pm = which('bun') ? 'bun' : which('npm') ? 'npm' : null;
  if (!pm) throw new Error('Need bun or npm on PATH to install the proxy (claude-code-router).');

  process.stdout.write(`\nInstalling the proxy (claude-code-router) with ${pm} — one time only…\n`);
  const pmPath = which(pm);
  const ext = path.extname(pmPath).toLowerCase();
  const installArgs = ['install', '-g', '@musistudio/claude-code-router'];
  let r;
  if (isWin && (ext === '.cmd' || ext === '.bat')) {
    const line = [pmPath, ...installArgs].map(winQuote).join(' ');
    r = spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', line], {
      stdio: 'inherit',
      windowsVerbatimArguments: true
    });
  } else {
    r = spawnSync(pmPath, installArgs, { stdio: 'inherit' });
  }
  if (r.status !== 0) throw new Error('Proxy install failed.');

  dirs = globalBinDirs();
  ccr = which('ccr', dirs);
  if (!ccr) throw new Error('Installed the proxy but could not locate the `ccr` binary. Add your global bin dir to PATH and retry.');
  return { ccr, dirs };
}
