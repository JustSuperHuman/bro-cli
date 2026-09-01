import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { BRO_DIR } from './config.js';

// Run JustImagine as a background service that starts itself at login and
// restarts itself if it dies — without ever asking for admin/root.
//
//   Windows  Task Scheduler task with a logon trigger, registered from XML.
//            (`schtasks /Create /SC ONLOGON` needs elevation; the same trigger
//            supplied as XML does not.) A wscript shim launches node with no
//            console window; without WSH we fall back to node directly.
//   macOS    launchd LaunchAgent in ~/Library/LaunchAgents (RunAtLoad+KeepAlive).
//   Linux    systemd --user unit (Restart=always), or an XDG autostart entry
//            on systems without systemd.

export const SERVICE_ID = 'justimagine';
export const SERVICE_LABEL = 'com.justgains.justimagine';
export const TASK_NAME = 'JustImagine';
export const DEFAULT_PORT = 8791;

export const SERVICE_DIR = path.join(BRO_DIR, 'justimagine');
export const SERVICE_CONFIG = path.join(SERVICE_DIR, 'service.json');
export const SERVICE_LOG = path.join(SERVICE_DIR, 'service.log');
export const SERVICE_ERR = path.join(SERVICE_DIR, 'service.err.log');
const VBS_SHIM = path.join(SERVICE_DIR, 'launch.vbs');

export const defaultServiceRoot = () => path.join(os.homedir(), 'JustImagine');

const broEntry = () => path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'bin', 'bro.js');

export function readServiceConfig() {
  try {
    return JSON.parse(fs.readFileSync(SERVICE_CONFIG, 'utf8'));
  } catch {
    return null;
  }
}

export function writeServiceConfig(cfg) {
  fs.mkdirSync(SERVICE_DIR, { recursive: true });
  fs.writeFileSync(SERVICE_CONFIG, JSON.stringify(cfg, null, 2));
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

// ---------- windows ----------

const winUser = () => `${process.env.USERDOMAIN || os.hostname()}\\${process.env.USERNAME || os.userInfo().username}`;
const xmlEscape = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export function windowsTaskXml({ command, args, cwd, user = winUser() }) {
  // ExecutionTimeLimit PT0S = never kill it. RestartOnFailure covers a crash;
  // StartWhenAvailable covers a machine that was asleep at logon.
  return `<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>JustImagine — image and video generation gallery (bro-cli)</Description>
    <URI>\\${TASK_NAME}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>${xmlEscape(user)}</UserId></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>${xmlEscape(user)}</UserId>
      <LogonType>InteractiveToken</LogonType>
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
// `node.exe` would otherwise flash on screen at every logon.
export function vbsShim({ node, entry, args }) {
  const quoted = [node, entry, ...args].map((a) => `""${a}""`).join(' ');
  return [
    'Set sh = CreateObject("WScript.Shell")',
    `sh.Run "${quoted}", 0, False`
  ].join('\r\n');
}

function windowsAction() {
  const node = process.execPath;
  const args = [broEntry(), 'imagine', 'service', 'run'];
  if (has('wscript')) {
    fs.mkdirSync(SERVICE_DIR, { recursive: true });
    fs.writeFileSync(VBS_SHIM, vbsShim({ node, entry: args[0], args: args.slice(1) }), 'ascii');
    return { command: 'wscript.exe', args: `//nologo "${VBS_SHIM}"`, hidden: true };
  }
  return { command: node, args: args.map((a) => `"${a}"`).join(' '), hidden: false };
}

const winInstall = (cfg) => {
  const { command, args, hidden } = windowsAction();
  const xmlPath = path.join(SERVICE_DIR, 'task.xml');
  fs.mkdirSync(SERVICE_DIR, { recursive: true });
  // schtasks insists on UTF-16 with a BOM for /XML input.
  fs.writeFileSync(xmlPath, Buffer.from('﻿' + windowsTaskXml({ command, args, cwd: cfg.root }), 'utf16le'));
  const r = run('schtasks', ['/Create', '/TN', TASK_NAME, '/XML', xmlPath, '/F']);
  if (!r.ok) return { ok: false, message: r.out || r.err };
  return {
    ok: true,
    mechanism: `Task Scheduler task "${TASK_NAME}" (at logon)${hidden ? '' : ' — WSH is unavailable, so a console window will appear at logon'}`
  };
};

const winUninstall = () => {
  const r = run('schtasks', ['/Delete', '/TN', TASK_NAME, '/F']);
  return { ok: r.ok || /cannot find/i.test(r.out + r.err), message: r.out || r.err };
};

const winStart = () => {
  const r = run('schtasks', ['/Run', '/TN', TASK_NAME]);
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

const winStop = () => {
  run('schtasks', ['/End', '/TN', TASK_NAME]);
  const cfg = readServiceConfig();
  if (!pidAlive(cfg?.pid)) return { ok: true, message: 'not running' };
  const r = run('taskkill', ['/PID', String(cfg.pid), '/T', '/F']);
  if (r.ok) writeServiceConfig({ ...cfg, pid: null });
  return { ok: r.ok, message: r.out || r.err };
};

const winStatus = () => {
  const r = run('schtasks', ['/Query', '/TN', TASK_NAME, '/FO', 'LIST']);
  if (!r.ok) return { installed: false, running: false, detail: 'not installed' };
  const running = pidAlive(readServiceConfig()?.pid);
  return { installed: true, running, detail: running ? 'running, starts at logon' : 'installed, starts at logon' };
};

// ---------- macos ----------

const plistPath = () => path.join(os.homedir(), 'Library', 'LaunchAgents', `${SERVICE_LABEL}.plist`);

export function launchdPlist({ node, entry, root, port }) {
  const argv = [node, entry, 'imagine', 'service', 'run'];
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
    <key>JUSTIMAGINE_ROOT</key><string>${xmlEscape(root)}</string>
    <key>JUSTIMAGINE_PORT</key><string>${port}</string>
  </dict>
  <key>WorkingDirectory</key><string>${xmlEscape(root)}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>${xmlEscape(SERVICE_LOG)}</string>
  <key>StandardErrorPath</key><string>${xmlEscape(SERVICE_ERR)}</string>
</dict>
</plist>
`;
}

const macTarget = () => `gui/${process.getuid?.() ?? ''}`;

const macInstall = (cfg) => {
  const file = plistPath();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.mkdirSync(SERVICE_DIR, { recursive: true });
  fs.writeFileSync(file, launchdPlist({ node: process.execPath, entry: broEntry(), root: cfg.root, port: cfg.port }));
  run('launchctl', ['bootout', `${macTarget()}/${SERVICE_LABEL}`]); // ignore "not loaded"
  let r = run('launchctl', ['bootstrap', macTarget(), file]);
  // launchctl bootstrap arrived in 10.11; older syntax still works everywhere.
  if (!r.ok) r = run('launchctl', ['load', '-w', file]);
  if (!r.ok) return { ok: false, message: r.err || r.out };
  return { ok: true, mechanism: `launchd agent ${SERVICE_LABEL} (${file})` };
};

const macUninstall = () => {
  const file = plistPath();
  run('launchctl', ['bootout', `${macTarget()}/${SERVICE_LABEL}`]);
  run('launchctl', ['unload', '-w', file]);
  try {
    fs.unlinkSync(file);
  } catch {
    /* already gone */
  }
  return { ok: true };
};

const macStart = () => {
  const r = run('launchctl', ['kickstart', '-k', `${macTarget()}/${SERVICE_LABEL}`]);
  return { ok: r.ok, message: r.err || r.out };
};

const macStop = () => {
  const r = run('launchctl', ['kill', 'SIGTERM', `${macTarget()}/${SERVICE_LABEL}`]);
  return { ok: r.ok, message: r.err || r.out };
};

const macStatus = () => {
  if (!fs.existsSync(plistPath())) return { installed: false, running: false, detail: 'not installed' };
  const r = run('launchctl', ['list', SERVICE_LABEL]);
  if (!r.ok) return { installed: true, running: false, detail: 'installed, not loaded' };
  const pid = (r.out.match(/"PID"\s*=\s*(\d+)/) || [])[1];
  return { installed: true, running: !!pid, detail: pid ? `running (pid ${pid})` : 'loaded, not running' };
};

// ---------- linux ----------

const unitPath = () => path.join(os.homedir(), '.config', 'systemd', 'user', `${SERVICE_ID}.service`);
const autostartPath = () => path.join(os.homedir(), '.config', 'autostart', `${SERVICE_ID}.desktop`);
const hasSystemd = () => has('systemctl') && fs.existsSync('/run/systemd/system');

export function systemdUnit({ node, entry, root, port }) {
  return `[Unit]
Description=JustImagine — image and video generation gallery (bro-cli)
After=network-online.target

[Service]
Type=simple
ExecStart=${node} ${entry} imagine service run
Environment=JUSTIMAGINE_ROOT=${root}
Environment=JUSTIMAGINE_PORT=${port}
WorkingDirectory=${root}
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
`;
}

export function autostartDesktop({ node, entry, root, port }) {
  return `[Desktop Entry]
Type=Application
Name=JustImagine
Comment=Image and video generation gallery (bro-cli)
Exec=env JUSTIMAGINE_ROOT=${root} JUSTIMAGINE_PORT=${port} ${node} ${entry} imagine service run
Terminal=false
X-GNOME-Autostart-enabled=true
`;
}

const linuxInstall = (cfg) => {
  const node = process.execPath;
  const entry = broEntry();
  fs.mkdirSync(SERVICE_DIR, { recursive: true });
  if (hasSystemd()) {
    const file = unitPath();
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, systemdUnit({ node, entry, root: cfg.root, port: cfg.port }));
    run('systemctl', ['--user', 'daemon-reload']);
    // Linger keeps the unit alive across logout on a headless box. It may need
    // a policy prompt, so a failure here is not fatal.
    run('loginctl', ['enable-linger', os.userInfo().username]);
    const r = run('systemctl', ['--user', 'enable', '--now', `${SERVICE_ID}.service`]);
    if (!r.ok) return { ok: false, message: r.err || r.out };
    return { ok: true, mechanism: `systemd user unit ${SERVICE_ID}.service (${file})` };
  }
  const file = autostartPath();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, autostartDesktop({ node, entry, root: cfg.root, port: cfg.port }));
  return { ok: true, mechanism: `XDG autostart entry (${file}) — no systemd here, so it starts at desktop login only` };
};

const linuxUninstall = () => {
  if (hasSystemd()) {
    run('systemctl', ['--user', 'disable', '--now', `${SERVICE_ID}.service`]);
    try {
      fs.unlinkSync(unitPath());
    } catch {
      /* already gone */
    }
    run('systemctl', ['--user', 'daemon-reload']);
  }
  try {
    fs.unlinkSync(autostartPath());
  } catch {
    /* already gone */
  }
  return { ok: true };
};

const linuxStart = () => {
  if (!hasSystemd()) return { ok: false, message: 'No systemd here — start it with `bro imagine service run &`.' };
  const r = run('systemctl', ['--user', 'restart', `${SERVICE_ID}.service`]);
  return { ok: r.ok, message: r.err || r.out };
};

const linuxStop = () => {
  if (!hasSystemd()) return { ok: false, message: 'No systemd here — stop the process yourself.' };
  const r = run('systemctl', ['--user', 'stop', `${SERVICE_ID}.service`]);
  return { ok: r.ok, message: r.err || r.out };
};

const linuxStatus = () => {
  if (hasSystemd()) {
    if (!fs.existsSync(unitPath())) return { installed: false, running: false, detail: 'not installed' };
    const active = run('systemctl', ['--user', 'is-active', `${SERVICE_ID}.service`]);
    return { installed: true, running: active.out === 'active', detail: active.out || 'unknown' };
  }
  const installed = fs.existsSync(autostartPath());
  return { installed, running: false, detail: installed ? 'autostart entry present (state unknown without systemd)' : 'not installed' };
};

// ---------- dispatch ----------

const BACKENDS = {
  win32: { install: winInstall, uninstall: winUninstall, start: winStart, stop: winStop, status: winStatus, name: 'Task Scheduler' },
  darwin: { install: macInstall, uninstall: macUninstall, start: macStart, stop: macStop, status: macStatus, name: 'launchd' },
  linux: { install: linuxInstall, uninstall: linuxUninstall, start: linuxStart, stop: linuxStop, status: linuxStatus, name: 'systemd --user' }
};

export const backendFor = (platform = process.platform) => BACKENDS[platform] || null;

export function serviceInstall(cfg) {
  const backend = backendFor();
  if (!backend) return { ok: false, message: `No service integration for ${process.platform}.` };
  fs.mkdirSync(cfg.root, { recursive: true });
  writeServiceConfig(cfg);
  return backend.install(cfg);
}

export const serviceUninstall = () => backendFor()?.uninstall() || { ok: false, message: `No service integration for ${process.platform}.` };
export const serviceStart = () => backendFor()?.start() || { ok: false, message: `No service integration for ${process.platform}.` };
export const serviceStop = () => backendFor()?.stop() || { ok: false, message: `No service integration for ${process.platform}.` };
export const serviceStatus = () => backendFor()?.status() || { installed: false, running: false, detail: `unsupported platform ${process.platform}` };

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
