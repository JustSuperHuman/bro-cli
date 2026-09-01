import { spawn, spawnSync, execSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { withTerminalAgent } from './terminal-agent.js';

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
  const dirs = [path.join(os.homedir(), '.bun', 'bin'), path.join(os.homedir(), '.local', 'bin')];
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

// cmd.exe /s /c strips the first and last quote from its command string. When
// the executable itself is quoted (for example C:\Program Files\nodejs\npm.cmd),
// the whole command therefore needs one additional outer quote pair.
export function windowsCmdLine(file, args) {
  return `"${[file, ...args].map(winQuote).join(' ')}"`;
}

// Spawn inheriting stdio. Handles Windows .cmd/.bat shims (npm), which can't be
// spawned directly. `cwd` runs the child in another directory (resuming a
// session from a different project) without moving bro's own process.
// Resolves with the child's exit code.
export function runInherit(file, args, env = process.env, { cwd, terminalAgent } = {}) {
  return withTerminalAgent(terminalAgent, () => new Promise((resolve) => {
    const ext = path.extname(file).toLowerCase();
    let child;
    if (isWin && (ext === '.cmd' || ext === '.bat')) {
      const line = windowsCmdLine(file, args);
      child = spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', line], {
        stdio: 'inherit',
        env,
        cwd,
        windowsVerbatimArguments: true
      });
    } else {
      child = spawn(file, args, { stdio: 'inherit', env, cwd });
    }
    child.on('exit', (code) => resolve(code ?? 0));
    child.on('error', (err) => {
      console.error(err.message);
      resolve(1);
    });
  }));
}

// Spawn an interactive command while mirroring stdout through this process.
// This is used by browser-backed harnesses whose stdout contains the exact URL
// to open. Stdin and stderr remain inherited, so the child still behaves like
// a normal foreground CLI.
export function runInheritObserved(file, args, env = process.env, { cwd, onStdout } = {}) {
  return new Promise((resolve) => {
    const ext = path.extname(file).toLowerCase();
    let child;
    const options = { stdio: ['inherit', 'pipe', 'inherit'], env, cwd };
    if (isWin && (ext === '.cmd' || ext === '.bat')) {
      const line = windowsCmdLine(file, args);
      child = spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', line], {
        ...options,
        windowsVerbatimArguments: true
      });
    } else {
      child = spawn(file, args, options);
    }
    child.stdout?.on('data', (chunk) => {
      process.stdout.write(chunk);
      onStdout?.(chunk.toString());
    });
    child.on('exit', (code) => resolve(code ?? 0));
    child.on('error', (err) => {
      console.error(err.message);
      resolve(1);
    });
  });
}

// Open an HTTP(S) URL with the operating system's default browser without
// holding the parent process open.
export function openExternalUrl(url) {
  let child;
  if (isWin) {
    child = spawn('rundll32', ['url.dll,FileProtocolHandler', url], { detached: true, stdio: 'ignore' });
  } else {
    child = spawn(process.platform === 'darwin' ? 'open' : 'xdg-open', [url], {
      detached: true,
      stdio: 'ignore'
    });
  }
  child.on('error', () => {});
  child.unref();
}

// Make sure the Anthropic<->OpenAI proxy (claude-code-router / `ccr`) is present,
// installing it with bun or npm if needed. Returns { ccr, dirs }.
export function ensureProxy() {
  let dirs = globalBinDirs();
  let ccr = which('ccr', dirs);
  if (ccr) return { ccr, dirs };

  const pm = which('bun') ? 'bun' : which('npm') ? 'npm' : null;
  if (!pm) throw new Error('Need bun or npm on PATH to install the proxy (claude-code-router).');

  process.stderr.write(`\nInstalling the proxy (claude-code-router) with ${pm} — one time only…\n`);
  const pmPath = which(pm);
  const ext = path.extname(pmPath).toLowerCase();
  const installArgs = ['install', '-g', '@musistudio/claude-code-router'];
  let r;
  if (isWin && (ext === '.cmd' || ext === '.bat')) {
    const line = windowsCmdLine(pmPath, installArgs);
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

function runSyncInherit(file, args) {
  const ext = path.extname(file).toLowerCase();
  if (isWin && (ext === '.cmd' || ext === '.bat')) {
    const line = windowsCmdLine(file, args);
    return spawnSync(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', line], {
      stdio: 'inherit',
      windowsVerbatimArguments: true
    });
  }
  return spawnSync(file, args, { stdio: 'inherit' });
}

// Every selectable coding harness can repair its own missing global command.
// npm is the broadest cross-platform path for the Node-based harnesses; Bun is
// a fallback (and omp's required runtime). Pi's upstream install explicitly
// recommends --ignore-scripts because it has no required lifecycle scripts.
export const HARNESS_INSTALLS = Object.freeze({
  claude: Object.freeze({
    label: 'Claude Code',
    command: 'claude',
    packageName: '@anthropic-ai/claude-code',
    managers: ['npm', 'bun']
  }),
  omp: Object.freeze({
    label: 'omp',
    command: 'omp',
    packageName: '@oh-my-pi/pi-coding-agent',
    managers: ['bun']
  }),
  pi: Object.freeze({
    label: 'Pi',
    command: 'pi',
    packageName: '@earendil-works/pi-coding-agent',
    managers: ['npm', 'bun'],
    installOptions: ['--ignore-scripts']
  }),
  codex: Object.freeze({
    label: 'Codex',
    command: 'codex',
    packageName: '@openai/codex',
    managers: ['npm', 'bun']
  }),
  dsh: Object.freeze({
    label: 'DeepSeek Harness',
    command: 'dsh',
    packageName: '@deepseek-ai/dsh',
    installTarget: '@deepseek-ai/dsh@latest',
    managers: ['npm', 'bun']
  })
});

export function globalInstallArgs(spec) {
  return ['install', '-g', ...(spec.installOptions || []), spec.installTarget || spec.packageName];
}

export function supportsDshNode(version = process.versions.node) {
  const [major = 0, minor = 0] = String(version).replace(/^v/, '').split('.').map(Number);
  return major >= 24 || (major === 22 && minor >= 19);
}

function assertHarnessRuntime(name) {
  if (name === 'dsh' && !supportsDshNode()) {
    throw new Error(
      `DeepSeek Harness requires Node.js 22.19.x or 24+ (current: ${process.version}). ` +
        'Update Node.js, then retry.'
    );
  }
}

// The small dependency-injection surface makes the install behavior testable
// without touching a developer's real global packages.
export function ensureGlobalPackage(spec, {
  binDirs = globalBinDirs,
  find = which,
  run = runSyncInherit,
  announce = (message) => process.stderr.write(message)
} = {}) {
  let dirs = binDirs();
  let executable = find(spec.command, dirs);
  if (executable) return { executable, dirs };

  let manager = '';
  let managerPath = '';
  for (const candidate of spec.managers) {
    const found = find(candidate, dirs);
    if (found) {
      manager = candidate;
      managerPath = found;
      break;
    }
  }
  if (!managerPath) {
    throw new Error(
      `Need ${spec.managers.join(' or ')} on PATH to install ${spec.label} (${spec.packageName}).`
    );
  }

  announce(`\nInstalling ${spec.label} with ${manager} — one time only…\n`);
  const result = run(managerPath, globalInstallArgs(spec));
  if (result.status !== 0) throw new Error(`${spec.label} install failed.`);

  dirs = binDirs();
  executable = find(spec.command, dirs);
  if (!executable) {
    throw new Error(
      `Installed ${spec.label} but could not locate the \`${spec.command}\` binary. Add your global bin directory to PATH and retry.`
    );
  }
  return { executable, dirs };
}

export function ensureHarnessTool(name, options) {
  assertHarnessRuntime(name);
  const spec = HARNESS_INSTALLS[name];
  if (!spec) throw new Error(`Unknown harness installer: ${name}`);
  return ensureGlobalPackage(spec, options);
}

// Explicitly reinstall the current npm dist-tag. Unlike ensureHarnessTool,
// this always invokes the package manager and therefore doubles as the update
// path for fast-moving preview harnesses such as dsh.
export function updateHarnessTool(name, {
  binDirs = globalBinDirs,
  find = which,
  run = runSyncInherit,
  announce = (message) => process.stderr.write(message)
} = {}) {
  assertHarnessRuntime(name);
  const spec = HARNESS_INSTALLS[name];
  if (!spec) throw new Error(`Unknown harness installer: ${name}`);
  const dirs = binDirs();
  let manager = '';
  let managerPath = '';
  for (const candidate of spec.managers) {
    const found = find(candidate, dirs);
    if (found) {
      manager = candidate;
      managerPath = found;
      break;
    }
  }
  if (!managerPath) {
    throw new Error(`Need ${spec.managers.join(' or ')} on PATH to update ${spec.label} (${spec.packageName}).`);
  }
  announce(`\nUpdating ${spec.label} with ${manager}…\n`);
  const result = run(managerPath, globalInstallArgs(spec));
  if (result.status !== 0) throw new Error(`${spec.label} update failed.`);
  const refreshedDirs = binDirs();
  const executable = find(spec.command, refreshedDirs);
  if (!executable) {
    throw new Error(
      `Updated ${spec.label} but could not locate the \`${spec.command}\` binary. Add your global bin directory to PATH and retry.`
    );
  }
  return { executable, dirs: refreshedDirs };
}

export function ensureClaude(options) {
  const { executable, dirs } = ensureHarnessTool('claude', options);
  return { claude: executable, dirs };
}

export function ensureCodex(options) {
  const { executable, dirs } = ensureHarnessTool('codex', options);
  return { codex: executable, dirs };
}

export function ensurePi(options) {
  const { executable, dirs } = ensureHarnessTool('pi', options);
  return { pi: executable, dirs };
}

export function ensureDsh(options) {
  const { executable, dirs } = ensureHarnessTool('dsh', options);
  return { dsh: executable, dirs };
}

export function ensureBun(options) {
  const { executable, dirs } = ensureGlobalPackage({
    label: 'Bun',
    command: 'bun',
    packageName: 'bun',
    managers: ['npm']
  }, options);
  return { bun: executable, dirs };
}

// Make sure Oh My Pi (`omp`) is available. Prefer Bun's package install because
// it is the upstream recommended cross-platform package path; fall back to the
// official install scripts when Bun is not present.
export function ensureOmp() {
  let dirs = globalBinDirs();
  let omp = which('omp', dirs);
  if (omp) return { omp, dirs };

  const bun = which('bun', dirs);
  if (bun) {
    const installed = ensureHarnessTool('omp');
    return { omp: installed.executable, dirs: installed.dirs };
  } else if (isWin) {
    const ps = which('powershell') || which('pwsh');
    if (!ps) throw new Error('Need Bun or PowerShell on PATH to install omp. See https://omp.sh/');
    process.stderr.write('\nInstalling omp with the official PowerShell installer — one time only…\n');
    const r = runSyncInherit(ps, ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-Command', 'irm https://omp.sh/install.ps1 | iex']);
    if (r.status !== 0) throw new Error('omp install failed.');
  } else {
    const sh = which('sh');
    const curl = which('curl');
    if (!sh || !curl) throw new Error('Need Bun, or sh + curl, on PATH to install omp. See https://omp.sh/');
    process.stderr.write('\nInstalling omp with the official shell installer — one time only…\n');
    const r = runSyncInherit(sh, ['-c', 'curl -fsSL https://omp.sh/install | sh']);
    if (r.status !== 0) throw new Error('omp install failed.');
  }

  dirs = globalBinDirs();
  omp = which('omp', dirs);
  if (!omp) throw new Error('Installed omp but could not locate the `omp` binary. Add your global bin dir to PATH and retry.');
  return { omp, dirs };
}
