import { afterEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {
  DEFAULT_PORT,
  SERVICE_LABEL,
  TASK_NAME,
  TASK_NAME_SYSTEM,
  autostartDesktop,
  backendFor,
  defaultServiceRoot,
  elevationHint,
  invokingUser,
  isElevated,
  launchdPlist,
  pidAlive,
  probe,
  serviceDirFor,
  systemdUnit,
  vbsShim,
  windowsTaskXml
} from './justimagine-service.js';

const NODE = process.platform === 'win32' ? 'C:\\Program Files\\nodejs\\node.exe' : '/usr/bin/node';
const ENTRY = path.join('opt', 'bro', 'bin', 'bro.js');

test('every supported platform has a backend, and the rest degrade cleanly', () => {
  for (const p of ['win32', 'darwin', 'linux']) {
    const b = backendFor(p);
    expect(typeof b.install).toBe('function');
    expect(typeof b.uninstall).toBe('function');
    expect(typeof b.start).toBe('function');
    expect(typeof b.stop).toBe('function');
    expect(typeof b.status).toBe('function');
    expect(b.name.length).toBeGreaterThan(0);
  }
  expect(backendFor('aix')).toBe(null);
});

test('the default gallery for a service is the one this directory already uses', () => {
  // Same folder `bro imagine` serves here, so installing the service in a
  // project keeps the gallery you were just looking at.
  expect(defaultServiceRoot('D:/work/site')).toBe(path.join('D:/work/site', '.bro', 'justimagine'));
  expect(defaultServiceRoot()).toBe(path.join(process.cwd(), '.bro', 'justimagine'));
  expect(DEFAULT_PORT).toBe(8791);
});

// ---------- privilege ----------

test('elevation is decided by the token, and the hint names the right ritual', () => {
  // No getuid on Windows, so the POSIX branch is false there; on POSIX a test
  // run is not root either. Either way this must not throw or guess.
  expect(typeof isElevated('linux')).toBe('boolean');
  expect(isElevated('linux')).toBe(process.platform === 'win32' ? false : process.getuid() === 0);
  expect(elevationHint('win32')).toContain('Administrator terminal');
  expect(elevationHint('darwin')).toContain('sudo bro imagine service install');
});

test('an unelevated install runs as, and stores under, this very user', () => {
  const who = invokingUser();
  expect(who.name).toBe(os.userInfo().username);
  expect(who.home).toBe(os.homedir());
  expect(serviceDirFor(who.home)).toBe(path.join(os.homedir(), '.bro', 'justimagine'));
});

// ---------- windows ----------

test('the scheduled task uses a logon trigger, least privilege and no time limit', () => {
  const xml = windowsTaskXml({ command: 'wscript.exe', args: '//nologo "C:\\x\\launch.vbs"', cwd: 'C:\\gallery', user: 'HOST\\me' });
  expect(xml.startsWith('<?xml version="1.0" encoding="UTF-16"?>')).toBe(true);
  expect(xml).toContain('<LogonTrigger>');
  expect(xml).toContain('<UserId>HOST\\me</UserId>');
  // LeastPrivilege is what lets this register without an admin prompt.
  expect(xml).toContain('<RunLevel>LeastPrivilege</RunLevel>');
  expect(xml).toContain('<LogonType>InteractiveToken</LogonType>');
  // PT0S means "never kill it" — a gallery service must outlive the default 3 days.
  expect(xml).toContain('<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>');
  expect(xml).toContain('<RestartOnFailure>');
  expect(xml).toContain('<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>');
  expect(xml).toContain(`<URI>\\${TASK_NAME}</URI>`);
  expect(xml).toContain('<Command>wscript.exe</Command>');
  expect(xml).toContain('<WorkingDirectory>C:\\gallery</WorkingDirectory>');
});

test('xml special characters in a path or user name are escaped', () => {
  const xml = windowsTaskXml({ command: 'C:\\a&b\\node.exe', args: '"x" <y>', cwd: 'C:\\a&b', user: 'HOST\\me&co' });
  expect(xml).toContain('C:\\a&amp;b\\node.exe');
  expect(xml).toContain('&quot;x&quot; &lt;y&gt;');
  expect(xml).toContain('HOST\\me&amp;co');
  expect(xml).not.toMatch(/&(?!amp;|quot;|lt;|gt;)/);
});

test('the vbs shim launches node with a hidden window and does not wait', () => {
  const vbs = vbsShim({ node: NODE, entry: ENTRY, args: ['imagine', 'service', 'run'] });
  expect(vbs).toContain('CreateObject("WScript.Shell")');
  // window style 0 = hidden, False = do not block
  expect(vbs).toMatch(/, 0, False$/);
  expect(vbs).toContain(`""${NODE}""`);
  expect(vbs).toContain('""imagine"" ""service"" ""run""');
  expect(vbs.includes('\r\n')).toBe(true);
});

// The shim detaches, so the scheduler reports the task as finished the instant
// it starts — the recorded pid is the only thing that knows if the server is up.
test('pidAlive distinguishes this process from a pid nobody is using', () => {
  expect(pidAlive(process.pid)).toBe(true);
  expect(pidAlive(0)).toBe(false);
  expect(pidAlive(null)).toBe(false);
  expect(pidAlive(undefined)).toBe(false);
  // 0x3FFFFFFF is far beyond any real pid on Windows, macOS or Linux
  expect(pidAlive(0x3fffffff)).toBe(false);
});

// ---------- macos ----------

test('the launch agent runs at load, stays alive and logs where status points', () => {
  const plist = launchdPlist({ node: NODE, entry: ENTRY, root: '/Users/me/JustImagine', port: 8791 });
  expect(plist).toContain(`<key>Label</key><string>${SERVICE_LABEL}</string>`);
  expect(plist).toContain('<key>RunAtLoad</key><true/>');
  expect(plist).toContain('<key>KeepAlive</key><true/>');
  expect(plist).toContain('<string>imagine</string>');
  expect(plist).toContain('<string>service</string>');
  expect(plist).toContain('<string>run</string>');
  expect(plist).toContain('<key>JUSTIMAGINE_ROOT</key><string>/Users/me/JustImagine</string>');
  expect(plist).toContain('<key>JUSTIMAGINE_PORT</key><string>8791</string>');
  expect(plist).toContain('<key>ProcessType</key><string>Background</string>');
});

// ---------- linux ----------

test('the systemd user unit restarts always and installs into the default target', () => {
  const unit = systemdUnit({ node: NODE, entry: ENTRY, root: '/home/me/JustImagine', port: 8791 });
  expect(unit).toContain(`ExecStart=${NODE} ${ENTRY} imagine service run`);
  expect(unit).toContain('Restart=always');
  expect(unit).toContain('Environment=JUSTIMAGINE_ROOT=/home/me/JustImagine');
  expect(unit).toContain('Environment=JUSTIMAGINE_PORT=8791');
  expect(unit).toContain('WantedBy=default.target');
});

test('the autostart fallback carries the same environment', () => {
  const desktop = autostartDesktop({ node: NODE, entry: ENTRY, root: '/home/me/JustImagine', port: 8791 });
  expect(desktop.startsWith('[Desktop Entry]')).toBe(true);
  expect(desktop).toContain('Type=Application');
  expect(desktop).toContain('JUSTIMAGINE_ROOT=/home/me/JustImagine');
  expect(desktop).toContain('imagine service run');
  expect(desktop).toContain('Terminal=false');
});

// ---------- probe ----------

let probeServer;
afterEach(async () => {
  if (probeServer) await new Promise((r) => probeServer.close(r));
  probeServer = null;
});

test('probe reports the gallery of whatever is answering, and null when nothing is', async () => {
  probeServer = http.createServer((req, res) => {
    if (req.url !== '/api/state') {
      res.writeHead(404);
      return res.end();
    }
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ root: '/tmp/gallery', title: 'JustImagine' }));
  });
  await new Promise((r) => probeServer.listen(0, '127.0.0.1', r));
  const port = probeServer.address().port;

  expect(await probe(port)).toEqual({ root: '/tmp/gallery', title: 'JustImagine' });
  await new Promise((r) => probeServer.close(r));
  probeServer = null;
  expect(await probe(port)).toBe(null);
});

test('probe treats a non-JustImagine server on the port as not ours', async () => {
  probeServer = http.createServer((req, res) => {
    res.writeHead(500);
    res.end('something else lives here');
  });
  await new Promise((r) => probeServer.listen(0, '127.0.0.1', r));
  expect(await probe(probeServer.address().port)).toBe(null);
});

// ---------- windows registration, for real ----------

// Registering a logon-triggered task is the one part that cannot be proven by
// generating a string: `schtasks /Create /SC ONLOGON` needs elevation, and the
// whole design rests on the XML form not needing it.
const onWindows = process.platform === 'win32' ? test : test.skip;

onWindows('a logon-triggered task registers, queries and deletes without admin', async () => {
  const { spawnSync } = await import('node:child_process');
  const name = 'JustImagineSelfTest';
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ji-task-'));
  const xmlPath = path.join(dir, 'task.xml');
  const user = `${process.env.USERDOMAIN}\\${process.env.USERNAME}`;
  const xml = windowsTaskXml({ command: 'cmd.exe', args: '/c exit', cwd: dir, user });
  fs.writeFileSync(xmlPath, Buffer.from('\ufeff' + xml, 'utf16le'));

  const sh = (args) => spawnSync('schtasks', args, { encoding: 'utf8', windowsHide: true });
  try {
    const created = sh(['/Create', '/TN', name, '/XML', xmlPath, '/F']);
    expect(`${created.stdout}${created.stderr}`).not.toMatch(/Access is denied/i);
    expect(created.status).toBe(0);

    const query = sh(['/Query', '/TN', name, '/FO', 'LIST']);
    expect(query.status).toBe(0);
    expect(query.stdout).toContain(name);
  } finally {
    sh(['/Delete', '/TN', name, '/F']);
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

// ---------- system scope: starts with the machine ----------

test('an elevated Windows install boots the task, as the user, without a password', () => {
  const xml = windowsTaskXml({
    command: 'wscript.exe',
    args: '//nologo "C:\\x\\launch-system.vbs"',
    cwd: 'C:\\gallery',
    user: 'HOST\\me',
    scope: 'system'
  });
  expect(xml).toContain('<BootTrigger>');
  expect(xml).toContain('<Delay>PT30S</Delay>');
  // S4U is what runs a task as this user before anyone has logged on, with no
  // stored credential — the whole reason elevation buys a boot start.
  expect(xml).toContain('<LogonType>S4U</LogonType>');
  expect(xml).toContain('<UserId>HOST\\me</UserId>');
  // A listening server does not need admin rights just because the install did.
  expect(xml).toContain('<RunLevel>LeastPrivilege</RunLevel>');
  expect(xml).toContain(`<URI>\\${TASK_NAME_SYSTEM}</URI>`);
  // The logon trigger stays too, so a boot start that failed is retried.
  expect(xml).toContain('<LogonTrigger>');
});

test('the user-scope task has no boot trigger and keeps its own name', () => {
  const xml = windowsTaskXml({ command: 'wscript.exe', args: '', cwd: 'C:\\g', user: 'HOST\\me' });
  expect(xml).not.toContain('<BootTrigger>');
  expect(xml).toContain('<LogonType>InteractiveToken</LogonType>');
  expect(xml).toContain(`<URI>\\${TASK_NAME}</URI>`);
  expect(TASK_NAME_SYSTEM).not.toBe(TASK_NAME);
});

test('both scopes carry a repeating trigger, because the shim detaches', () => {
  for (const scope of ['user', 'system']) {
    const xml = windowsTaskXml({ command: 'x', args: '', cwd: 'C:\\g', user: 'HOST\\me', scope });
    expect(xml).toContain('<Repetition><Interval>PT5M</Interval>');
    // Repetition has to precede Enabled or the schema rejects the trigger.
    expect(xml.indexOf('<Repetition>')).toBeLessThan(xml.indexOf('<Enabled>true</Enabled>'));
  }
});

test('the shim can hand the service an environment the scheduler cannot', () => {
  const vbs = vbsShim({ node: NODE, entry: ENTRY, args: ['imagine'], env: { USERPROFILE: 'C:\\Users\\me' } });
  expect(vbs).toContain('sh.Environment("PROCESS")("USERPROFILE") = "C:\\Users\\me"');
  // The launch stays the last line, still hidden and still not waiting.
  expect(vbs.split('\r\n').pop()).toMatch(/, 0, False$/);
});

test('a launch daemon runs as the user, with their home, logging where they can read it', () => {
  const plist = launchdPlist({
    node: NODE,
    entry: ENTRY,
    root: '/Users/me/Art',
    port: 8791,
    scope: 'system',
    user: 'me',
    home: '/Users/me',
    logDir: '/Users/me/.bro/justimagine'
  });
  expect(plist).toContain('<key>UserName</key><string>me</string>');
  // Without HOME a daemon reads root's ~/.bro/config.json and finds no keys.
  expect(plist).toContain('<key>HOME</key><string>/Users/me</string>');
  expect(plist).toContain('<string>/Users/me/.bro/justimagine/service.log</string>');
  expect(plist).toContain('<key>RunAtLoad</key><true/>');
  expect(plist).toContain('<key>KeepAlive</key><true/>');
  // The gallery and port are on the command line, not just in the environment.
  expect(plist).toContain('<string>--root</string>');
  expect(plist).toContain('<string>/Users/me/Art</string>');
  expect(plist).toContain('<string>--port</string>');
});

test('a launch agent stays the user`s own, with no UserName and no borrowed home', () => {
  const plist = launchdPlist({ node: NODE, entry: ENTRY, root: '/Users/me/Art', port: 8791 });
  expect(plist).not.toContain('<key>UserName</key>');
  expect(plist).not.toContain('<key>HOME</key>');
});

test('a system unit runs as the user and is wanted by multi-user, not the session', () => {
  const unit = systemdUnit({
    node: NODE,
    entry: ENTRY,
    root: '/home/me/Art',
    port: 8791,
    scope: 'system',
    user: 'me',
    home: '/home/me'
  });
  expect(unit).toContain('User=me');
  expect(unit).toContain('Environment=HOME=/home/me');
  expect(unit).toContain('WantedBy=multi-user.target');
  expect(unit).toContain('--root /home/me/Art --port 8791');
  expect(unit).toContain('Restart=always');
});

test('a user unit names no user and stays in the login session', () => {
  const unit = systemdUnit({ node: NODE, entry: ENTRY, root: '/home/me/Art', port: 8791 });
  expect(unit).not.toContain('User=');
  expect(unit).not.toContain('Environment=HOME=');
  expect(unit).toContain('WantedBy=default.target');
});
