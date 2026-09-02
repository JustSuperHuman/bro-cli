import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';

// Run JustImagine as a background service. There are two scopes, and which one
// you get depends only on whether you were elevated when you installed it:
//
//   user    (no admin/root)  starts when you log on, stops when you log out.
//   system  (admin/root)     starts with the machine, before anyone logs on.
//
// The system install always wins: installing elevated retires the per-user one
// first, because two servers cannot share a port. Installing unelevated over a
// system install changes nothing and says so.
//
//   Windows  Task Scheduler task registered from XML — a logon trigger for the
//            user scope (`/SC ONLOGON` needs elevation; the same trigger as XML
//            does not), a boot trigger with an S4U principal for the system
//            scope, which is how a task runs as you without your password.
//            Both go through a wscript shim so no console window ever appears.
//   macOS    launchd: a LaunchAgent in ~/Library/LaunchAgents, or a LaunchDaemon
//            in /Library/LaunchDaemons that runs as you (RunAtLoad + KeepAlive).
//   Linux    systemd: a --user unit, or a system unit with User=you. Without
//            systemd, an XDG autostart entry (user scope only).

export const SERVICE_ID = 'justimagine';
export const SERVICE_LABEL = 'com.justgains.justimagine';
export const TASK_NAME = 'JustImagine';
export const TASK_NAME_SYSTEM = 'JustImagine-System';
export const DEFAULT_PORT = 8791;

export const SERVICE_DIR = path.join(BRO_DIR, 'justimagine');
export const SERVICE_CONFIG = path.join(SERVICE_DIR, 'service.json');
export const SERVICE_LOG = path.join(SERVICE_DIR, 'service.log');
export const SERVICE_ERR = path.join(SERVICE_DIR, 'service.err.log');

// Where a given home directory keeps its service files. Only interesting when
// installing elevated: `sudo` may hand us root's home, but the service runs as
// the user and must log where that user — and a later unelevated `service logs`
// — can find it.
export const serviceDirFor = (home) => path.join(home, '.bro', 'justimagine');

// The gallery a fresh install serves: the same folder `bro imagine` uses in this
// directory, so installing the service where you have been working keeps the
// gallery you were just looking at rather than starting an empty one elsewhere.
export const defaultServiceRoot = (cwd = process.cwd()) => path.join(cwd, '.bro', 'justimagine');

const broEntry = () => path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'bin', 'bro.js');

// The service is told its gallery and port on the command line rather than left
// to find them: a boot-time process has no cwd worth trusting and, on Windows,
// no environment we can set through the scheduler.
const runArgs = (cfg) => ['imagine', 'service', 'run', '--root', cfg.root, '--port', String(cfg.port)];

export function readServiceConfig(file = SERVICE_CONFIG) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

export function writeServiceConfig(cfg, file = SERVICE_CONFIG) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, JSON.stringify(cfg, null, 2));
  return cfg;
}

// Best-effort process runner: services fail in readable ways rather than
// throwing raw ENOENT at the user.
function run(cmd, args, { input } = {}) {
  const r = spawnSync(cmd, args, { encoding: 'utf8', input, windowsHide: true });
  return {
    ok: r.status === 0,
    code: r.status,
    out: (r.stdout || '').trim(),
    err: (r.error?.message || r.stderr || '').trim()
  };
}

const has = (cmd) => {
  const probe = process.platform === 'win32' ? run('where', [cmd]) : run('which', [cmd]);
  return probe.ok && !!probe.out;
};

// ---------- privilege ----------

// Can this process register something that starts before login? On POSIX that is
// uid 0. On Windows it is the token's integrity level — group membership only
// says the account *could* elevate, not that this window did.
export function isElevated(platform = process.platform) {
  if (platform !== 'win32') return typeof process.getuid === 'function' && process.getuid() === 0;
  const groups = run('whoami', ['/groups']);
  // S-1-16-12288 = High integrity, S-1-16-16384 = System. SIDs are printed on
  // every locale, which the group names are not.
  if (groups.ok) return /S-1-16-(12288|16384)\b/.test(groups.out);
  return run('net', ['session']).ok; // no whoami: this only succeeds elevated
}

export const elevationHint = (platform = process.platform) =>
  platform === 'win32'
    ? 'To start it at boot instead, re-run this in an Administrator terminal (Win+X → Terminal (Admin)).'
    : 'To start it at boot instead, re-run it with sudo: sudo bro imagine service install';

// Who the service should run as, and whose home holds the config and the keys.
// Under sudo that is not this process: sudo may hand us root's home, and a
// service running as root would read the wrong ~/.bro/config.json.
export function invokingUser() {
  const me = (() => {
    try {
      return os.userInfo();
    } catch {
      return { username: process.env.USERNAME || process.env.USER || 'unknown', homedir: os.homedir(), uid: -1 };
    }
  })();
  const name = process.env.SUDO_USER || process.env.DOAS_USER || me.username;
  if (name === me.username) return { name, home: me.homedir, uid: me.uid };
  // A sudo'd install: ask the system where that account lives rather than
  // guessing /home/<name>, which is wrong on macOS and on any custom layout.
  const uid = Number(run('id', ['-u', name]).out) || me.uid;
  const home =
    process.platform === 'darwin'
      ? (run('dscl', ['.', '-read', `/Users/${name}`, 'NFSHomeDirectory']).out.split(': ')[1] || `/Users/${name}`).trim()
      : (run('getent', ['passwd', name]).out.split(':')[5] || `/home/${name}`).trim();
  return { name, home, uid };
}

// ---------- windows ----------

const winUser = () => `${process.env.USERDOMAIN || os.hostname()}\\${process.env.USERNAME || os.userInfo().username}`;
const xmlEscape = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const taskNameFor = (scope) => (scope === 'system' ? TASK_NAME_SYSTEM : TASK_NAME);

export function windowsTaskXml({ command, args, cwd, user = winUser(), scope = 'user' }) {
  const system = scope === 'system';
  // The shim detaches, so the scheduler thinks the task finished the moment it
  // starts and RestartOnFailure never fires. A repeating trigger is the real
  // keepalive: every five minutes it tries again, and an attempt that finds the
  // port already answering exits quietly.
  const repetition = '<Repetition><Interval>PT5M</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition>';
  const logon = `<LogonTrigger>${repetition}<Enabled>true</Enabled><UserId>${xmlEscape(user)}</UserId></LogonTrigger>`;
  const triggers = system
    ? `    <BootTrigger>${repetition}<Enabled>true</Enabled><Delay>PT30S</Delay></BootTrigger>\n    ${logon}`
    : `    ${logon}`;
  // S4U runs the task as this user with no stored password, which is what lets
  // an elevated install start before anyone logs on without asking for one.
  return `<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>JustImagine — image and video generation gallery (bro-cli)</Description>
    <URI>\\${taskNameFor(scope)}</URI>
  </RegistrationInfo>
  <Triggers>
${triggers}
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>${xmlEscape(user)}</UserId>
      <LogonType>${system ? 'S4U' : 'InteractiveToken'}</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>${xmlEscape(command)}</Command>
      <Arguments>${xmlEscape(args)}</Arguments>
      <WorkingDirectory>${xmlEscape(cwd)}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>`;
}

// wscript runs a process with no console window at all, which a scheduled
// node.exe would otherwise flash on screen at every logon. It is also the only
// place a Windows service can be handed an environment: Task Scheduler's XML has
// nowhere to put one, and a boot-time S4U task may otherwise resolve
// %USERPROFILE% to the system profile and read the wrong config file.
export function vbsShim({ node, entry, args, env = {} }) {
  const quoted = [node, entry, ...args].map((a) => `""${a}""`).join(' ');
  return [
    'Set sh = CreateObject("WScript.Shell")',
    ...Object.entries(env).map(([k, v]) => `sh.Environment("PROCESS")("${k}") = "${String(v).replace(/"/g, '""')}"`),
    `sh.Run "${quoted}", 0, False`
  ].join('\r\n');
}

function windowsAction(cfg, scope) {
  const node = process.execPath;
  const entry = broEntry();
  const args = runArgs(cfg);
  if (has('wscript')) {
    const shim = path.join(SERVICE_DIR, scope === 'system' ? 'launch-system.vbs' : 'launch.vbs');
    fs.mkdirSync(SERVICE_DIR, { recursive: true });
    fs.writeFileSync(shim, vbsShim({ node, entry, args, env: { USERPROFILE: os.homedir() } }), 'ascii');
    return { command: 'wscript.exe', args: `//nologo "${shim}"`, hidden: true };
  }
  return { command: node, args: [entry, ...args].map((a) => `"${a}"`).join(' '), hidden: false };
}

const winInstall = (cfg) => {
  const scope = cfg.scope === 'system' ? 'system' : 'user';
  const { command, args, hidden } = windowsAction(cfg, scope);
  const xmlPath = path.join(SERVICE_DIR, scope === 'system' ? 'task-system.xml' : 'task.xml');
  fs.mkdirSync(SERVICE_DIR, { recursive: true });
  // schtasks insists on UTF-16 with a BOM for /XML input.
  fs.writeFileSync(xmlPath, Buffer.from('﻿' + windowsTaskXml({ command, args, cwd: cfg.root, scope }), 'utf16le'));
  const r = run('schtasks', ['/Create', '/TN', taskNameFor(scope), '/XML', xmlPath, '/F']);
  if (!r.ok) return { ok: false, message: r.out || r.err };
  const when = scope === 'system' ? 'at boot, whether or not you are logged on' : 'at logon';
  return {
    ok: true,
    scope,
    mechanism: `Task Scheduler task "${taskNameFor(scope)}" (${when})${hidden ? '' : ' — WSH is unavailable, so a console window will appear'}`
  };
};

const winUninstall = ({ scope } = {}) => {
  const problems = [];
  for (const s of scope ? [scope] : ['system', 'user']) {
    const r = run('schtasks', ['/Delete', '/TN', taskNameFor(s), '/F']);
    if (!r.ok && !/cannot find|does not exist/i.test(r.out + r.err)) problems.push(`${s}: ${r.out || r.err}`);
  }
  return { ok: !problems.length, message: problems.join('; ') };
};

const winStart = ({ scope } = {}) => {
  const r = run('schtasks', ['/Run', '/TN', taskNameFor(scope || winStatus().scope)]);
  return { ok: r.ok, message: r.out || r.err };
};

// The wscript shim launches node detached and exits, so the scheduler considers
// the task finished the moment it starts — `schtasks /End` has nothing left to
// end. The server records its own pid instead, and that is what gets stopped.
export function pidAlive(pid) {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (e) {
    return e.code === 'EPERM';
  }
}

const winStop = ({ scope } = {}) => {
  for (const s of scope ? [scope] : ['system', 'user']) run('schtasks', ['/End', '/TN', taskNameFor(s)]);
  const cfg = readServiceConfig();
  if (!pidAlive(cfg?.pid)) return { ok: true, message: 'not running' };
  const r = run('taskkill', ['/PID', String(cfg.pid), '/T', '/F']);
  if (r.ok) writeServiceConfig({ ...cfg, pid: null });
  return { ok: r.ok, message: r.out || r.err };
};

const winStatus = () => {
  for (const scope of ['system', 'user']) {
    if (!run('schtasks', ['/Query', '/TN', taskNameFor(scope), '/FO', 'LIST']).ok) continue;
    const running = pidAlive(readServiceConfig()?.pid);
    const when = scope === 'system' ? 'starts at boot' : 'starts at logon';
    return { installed: true, scope, running, detail: `${running ? 'running' : 'installed'}, ${when}` };
  }
  return { installed: false, running: false, detail: 'not installed' };
};

// ---------- macos ----------

const plistPath = (scope, home = invokingUser().home) =>
  scope === 'system'
    ? path.join('/Library', 'LaunchDaemons', `${SERVICE_LABEL}.plist`)
    : path.join(home, 'Library', 'LaunchAgents', `${SERVICE_LABEL}.plist`);

export function launchdPlist({ node, entry, root, port, scope = 'user', user = '', home = '', logDir = SERVICE_DIR }) {
  const argv = [node, entry, ...runArgs({ root, port })];
  const env = { JUSTIMAGINE_ROOT: root, JUSTIMAGINE_PORT: String(port), ...(home ? { HOME: home } : {}) };
  const runAs = scope === 'system' && user ? `  <key>UserName</key><string>${xmlEscape(user)}</string>\n  <key>GroupName</key><string>staff</string>\n` : '';
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>${SERVICE_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
${argv.map((a) => `    <string>${xmlEscape(a)}</string>`).join('\n')}
  </array>
  <key>EnvironmentVariables</key>
  <dict>
${Object.entries(env)
  .map(([k, v]) => `    <key>${k}</key><string>${xmlEscape(v)}</string>`)
  .join('\n')}
  </dict>
${runAs}  <key>WorkingDirectory</key><string>${xmlEscape(root)}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>${xmlEscape(`${logDir}/service.log`)}</string>
  <key>StandardErrorPath</key><string>${xmlEscape(`${logDir}/service.err.log`)}</string>
</dict>
</plist>
`;
}

const macTarget = (scope, uid) => (scope === 'system' ? 'system' : `gui/${uid ?? process.getuid?.() ?? ''}`);

const macInstall = (cfg) => {
  const scope = cfg.scope === 'system' ? 'system' : 'user';
  const who = invokingUser();
  const file = plistPath(scope, who.home);
  const logDir = serviceDirFor(who.home);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.mkdirSync(logDir, { recursive: true });
  fs.writeFileSync(
    file,
    launchdPlist({
      node: process.execPath,
      entry: broEntry(),
      root: cfg.root,
      port: cfg.port,
      scope,
      user: who.name,
      home: who.home,
      logDir
    })
  );
  if (scope === 'system') {
    // launchd refuses to load a daemon plist that is not root-owned.
    fs.chmodSync(file, 0o644);
    run('chown', ['root:wheel', file]);
  }
  const target = macTarget(scope, who.uid);
  run('launchctl', ['bootout', `${target}/${SERVICE_LABEL}`]); // ignore "not loaded"
  let r = run('launchctl', ['bootstrap', target, file]);
  // launchctl bootstrap arrived in 10.11; the older syntax still works everywhere.
  if (!r.ok) r = run('launchctl', ['load', '-w', file]);
  if (!r.ok) return { ok: false, message: r.err || r.out };
  return {
    ok: true,
    scope,
    mechanism:
      scope === 'system'
        ? `launchd daemon ${SERVICE_LABEL} (${file}), at boot, running as ${who.name}`
        : `launchd agent ${SERVICE_LABEL} (${file}), at login`
  };
};

const macUninstall = ({ scope } = {}) => {
  const who = invokingUser();
  const problems = [];
  for (const s of scope ? [scope] : ['system', 'user']) {
    const file = plistPath(s, who.home);
    if (!fs.existsSync(file)) continue;
    run('launchctl', ['bootout', `${macTarget(s, who.uid)}/${SERVICE_LABEL}`]);
    run('launchctl', ['unload', '-w', file]);
    try {
      fs.unlinkSync(file);
    } catch (e) {
      problems.push(`${file}: ${e.code === 'EACCES' || e.code === 'EPERM' ? 'needs sudo' : e.message}`);
    }
  }
  return { ok: !problems.length, message: problems.join('; ') };
};

const macStart = ({ scope } = {}) => {
  const r = run('launchctl', ['kickstart', '-k', `${macTarget(scope || macStatus().scope, invokingUser().uid)}/${SERVICE_LABEL}`]);
  return { ok: r.ok, message: r.err || r.out };
};

const macStop = ({ scope } = {}) => {
  const r = run('launchctl', ['kill', 'SIGTERM', `${macTarget(scope || macStatus().scope, invokingUser().uid)}/${SERVICE_LABEL}`]);
  return { ok: r.ok, message: r.err || r.out };
};

const macStatus = () => {
  const who = invokingUser();
  for (const scope of ['system', 'user']) {
    if (!fs.existsSync(plistPath(scope, who.home))) continue;
    const when = scope === 'system' ? 'starts at boot' : 'starts at login';
    const r = run('launchctl', ['list', SERVICE_LABEL]);
    if (!r.ok) {
      // A system daemon is not in an unprivileged user's launchd domain, so
      // "cannot see it" is not "not running" — the port probe decides that.
      return {
        installed: true,
        scope,
        running: false,
        detail: scope === 'system' ? `installed, ${when} (state needs sudo to read)` : 'installed, not loaded'
      };
    }
    const pid = (r.out.match(/"PID"\s*=\s*(\d+)/) || [])[1];
    return { installed: true, scope, running: !!pid, detail: pid ? `running (pid ${pid}), ${when}` : `loaded, ${when}` };
  }
  return { installed: false, running: false, detail: 'not installed' };
};

// ---------- linux ----------

const unitPath = (scope, home = invokingUser().home) =>
  scope === 'system'
    ? path.join('/etc', 'systemd', 'system', `${SERVICE_ID}.service`)
    : path.join(home, '.config', 'systemd', 'user', `${SERVICE_ID}.service`);
const autostartPath = (home = invokingUser().home) => path.join(home, '.config', 'autostart', `${SERVICE_ID}.desktop`);
const hasSystemd = () => has('systemctl') && fs.existsSync('/run/systemd/system');

export function systemdUnit({ node, entry, root, port, scope = 'user', user = '', home = '' }) {
  const system = scope === 'system';
  const runAs = `${system && user ? `User=${user}\n` : ''}${system && home ? `Environment=HOME=${home}\n` : ''}`;
  return `[Unit]
Description=JustImagine — image and video generation gallery (bro-cli)
After=network-online.target

[Service]
Type=simple
ExecStart=${node} ${entry} ${runArgs({ root, port }).join(' ')}
${runAs}Environment=JUSTIMAGINE_ROOT=${root}
Environment=JUSTIMAGINE_PORT=${port}
WorkingDirectory=${root}
Restart=always
RestartSec=3

[Install]
WantedBy=${system ? 'multi-user.target' : 'default.target'}
`;
}

export function autostartDesktop({ node, entry, root, port }) {
  return `[Desktop Entry]
Type=Application
Name=JustImagine
Comment=Image and video generation gallery (bro-cli)
Exec=env JUSTIMAGINE_ROOT=${root} JUSTIMAGINE_PORT=${port} ${node} ${entry} ${runArgs({ root, port }).join(' ')}
Terminal=false
X-GNOME-Autostart-enabled=true
`;
}

const linuxInstall = (cfg) => {
  const scope = cfg.scope === 'system' ? 'system' : 'user';
  const node = process.execPath;
  const entry = broEntry();
  const who = invokingUser();
  fs.mkdirSync(serviceDirFor(who.home), { recursive: true });
  if (hasSystemd()) {
    const file = unitPath(scope, who.home);
    const ctl = scope === 'system' ? [] : ['--user'];
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, systemdUnit({ node, entry, root: cfg.root, port: cfg.port, scope, user: who.name, home: who.home }));
    run('systemctl', [...ctl, 'daemon-reload']);
    // Linger keeps a --user unit alive across logout on a headless box. It may
    // need a policy prompt, so a failure here is not fatal.
    if (scope === 'user') run('loginctl', ['enable-linger', who.name]);
    const r = run('systemctl', [...ctl, 'enable', '--now', `${SERVICE_ID}.service`]);
    if (!r.ok) return { ok: false, message: r.err || r.out };
    return {
      ok: true,
      scope,
      mechanism:
        scope === 'system'
          ? `systemd unit ${SERVICE_ID}.service (${file}), at boot, running as ${who.name}`
          : `systemd user unit ${SERVICE_ID}.service (${file}), at login`
    };
  }
  const file = autostartPath(who.home);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, autostartDesktop({ node, entry, root: cfg.root, port: cfg.port }));
  return {
    ok: true,
    scope: 'user',
    mechanism: `XDG autostart entry (${file}) — no systemd here, so it starts at desktop login only`
  };
};

// Reach into the invoking user's systemd session from a root install, so an
// elevated install can retire the --user unit it replaces.
const asUser = (who, argv) => run('sudo', ['-u', who.name, 'env', `XDG_RUNTIME_DIR=/run/user/${who.uid}`, ...argv]);

const linuxUninstall = ({ scope } = {}) => {
  const who = invokingUser();
  const problems = [];
  for (const s of scope ? [scope] : ['system', 'user']) {
    const file = unitPath(s, who.home);
    if (hasSystemd() && fs.existsSync(file)) {
      const off = ['disable', '--now', `${SERVICE_ID}.service`];
      if (s === 'system') run('systemctl', off);
      else if (process.getuid?.() === 0) asUser(who, ['systemctl', '--user', ...off]);
      else run('systemctl', ['--user', ...off]);
      try {
        fs.unlinkSync(file);
      } catch (e) {
        problems.push(`${file}: ${e.code === 'EACCES' || e.code === 'EPERM' ? 'needs sudo' : e.message}`);
      }
    }
    if (s === 'user') {
      try {
        fs.unlinkSync(autostartPath(who.home));
      } catch {
        /* already gone */
      }
    }
  }
  if (hasSystemd()) run('systemctl', ['daemon-reload']);
  return { ok: !problems.length, message: problems.join('; ') };
};

const linuxCtl = (verb, scope) => {
  if (!hasSystemd()) return { ok: false, message: `No systemd here — ${verb} it with \`bro imagine service run &\`.` };
  const s = scope || linuxStatus().scope || 'user';
  const r = run('systemctl', s === 'system' ? [verb, `${SERVICE_ID}.service`] : ['--user', verb, `${SERVICE_ID}.service`]);
  return { ok: r.ok, message: r.err || r.out };
};

const linuxStart = ({ scope } = {}) => linuxCtl('restart', scope);
const linuxStop = ({ scope } = {}) => linuxCtl('stop', scope);

const linuxStatus = () => {
  const who = invokingUser();
  if (hasSystemd()) {
    for (const scope of ['system', 'user']) {
      if (!fs.existsSync(unitPath(scope, who.home))) continue;
      const query = ['is-active', `${SERVICE_ID}.service`];
      const active = run('systemctl', scope === 'system' ? query : ['--user', ...query]);
      const when = scope === 'system' ? 'starts at boot' : 'starts at login';
      return { installed: true, scope, running: active.out === 'active', detail: `${active.out || 'unknown'}, ${when}` };
    }
  }
  const installed = fs.existsSync(autostartPath(who.home));
  return {
    installed,
    scope: installed ? 'user' : undefined,
    running: false,
    detail: installed ? 'autostart entry present (state unknown without systemd)' : 'not installed'
  };
};

// ---------- dispatch ----------

const BACKENDS = {
  win32: { install: winInstall, uninstall: winUninstall, start: winStart, stop: winStop, status: winStatus, name: 'Task Scheduler' },
  darwin: { install: macInstall, uninstall: macUninstall, start: macStart, stop: macStop, status: macStatus, name: 'launchd' },
  linux: { install: linuxInstall, uninstall: linuxUninstall, start: linuxStart, stop: linuxStop, status: linuxStatus, name: 'systemd' }
};

export const backendFor = (platform = process.platform) => BACKENDS[platform] || null;

const unsupported = () => ({ ok: false, message: `No service integration for ${process.platform}.` });

// Install, at the strongest scope this process is allowed to use. Elevated wins
// outright and overwrites whatever was there; unelevated never demotes a
// boot-time install it cannot replace.
export function serviceInstall(cfg) {
  const backend = backendFor();
  if (!backend) return unsupported();
  const elevated = isElevated();
  const scope = elevated ? 'system' : 'user';
  const before = backend.status();

  if (!elevated && before.installed && before.scope === 'system') {
    const saved = readServiceConfig() || {};
    const moved = saved.root && path.resolve(saved.root) !== path.resolve(cfg.root);
    return {
      ok: true,
      skipped: true,
      scope: 'system',
      elevated,
      installed: before,
      message:
        'A system-wide JustImagine is already installed and starts at boot; it takes priority, so nothing was changed.' +
        (moved ? `\n   It serves ${saved.root} — re-run elevated to point it at ${cfg.root}.` : '')
    };
  }

  const who = invokingUser();
  fs.mkdirSync(cfg.root, { recursive: true });
  const record = { ...cfg, scope, installedBy: who.name, installedAt: new Date().toISOString() };
  writeServiceConfig(record);
  // Under sudo our own ~/.bro is root's. The service runs as the user, and a
  // later unelevated `service status` reads the user's copy, so write both.
  const mine = serviceDirFor(who.home);
  if (path.resolve(mine) !== path.resolve(SERVICE_DIR)) writeServiceConfig(record, path.join(mine, 'service.json'));

  // Two servers cannot share a port, so the per-user install is retired before
  // the boot-time one takes over.
  const replaced = elevated && before.installed && before.scope === 'user';
  if (replaced) {
    backend.stop({ scope: 'user' });
    backend.uninstall({ scope: 'user' });
  }

  const r = backend.install(record);
  if (!r.ok) return { ...r, scope, elevated };
  if (elevated && process.platform !== 'win32') {
    // Whatever the install just created belongs to the user it runs as.
    run('chown', ['-R', String(who.uid), cfg.root]);
    run('chown', ['-R', String(who.uid), mine]);
  }
  return { ...r, scope, elevated, replaced, boot: scope === 'system' };
}

export const serviceUninstall = (opts) => backendFor()?.uninstall(opts || {}) || unsupported();
export const serviceStart = (opts) => backendFor()?.start(opts || {}) || unsupported();
export const serviceStop = (opts) => backendFor()?.stop(opts || {}) || unsupported();
export const serviceStatus = () =>
  backendFor()?.status() || { installed: false, running: false, detail: `unsupported platform ${process.platform}` };

// Is something already answering on the service port? Cheaper and more honest
// than the scheduler's own idea of "running" — this proves the UI is reachable.
export async function probe(port, timeoutMs = 1200) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/state`, { signal: ctrl.signal, headers: { connection: 'close' } });
    if (!res.ok) return null;
    const json = await res.json();
    return { root: json.root, title: json.title };
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}
