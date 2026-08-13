// Shared machinery for bro's two session switchers.
//
// Claude Code and Codex both keep their state in one directory per login — a
// Claude profile is a CLAUDE_CONFIG_DIR, a Codex profile is a CODEX_HOME — and
// both harnesses resume a conversation by id from inside that directory. So the
// switchers differ only in where the files sit: the rows the picker shows, the
// question "which login should resume this?", and the disposable copy that lets
// one login fork another's session are the same in both, and live here.

import fs from 'node:fs';
import path from 'node:path';
import { select } from './ui.js';
import { sessionLabel } from './sessions.js';

// One directory per profile, named after the profile.
export function listProfileDirs(root) {
  try {
    return fs
      .readdirSync(root, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => ({ name: d.name, dir: path.join(root, d.name) }))
      .sort((a, b) => a.name.localeCompare(b.name));
  } catch {
    return [];
  }
}

// Profile names become directory names, so they may not reach outside the
// profile root or collide with a path separator.
export function assertProfileName(name) {
  const value = String(name || '').trim();
  if (!value || !/^[A-Za-z0-9._-]+$/.test(value) || value === '.' || value === '..') {
    throw new Error(`Invalid profile name: ${name || '(empty)'}  (letters, digits, dot, dash and underscore only)`);
  }
  return value;
}

// The resumable sessions under a picker's profile rows: this project's first,
// then every other project's with its path, each row carrying the profile that
// owns it. Returns [] when there is no history, so callers can spread it.
export function sessionRows(sessions, toValue) {
  const rows = [];
  const toRow = (s, showPath) => ({
    label: sessionLabel(s, { showPath }),
    value: toValue(s),
    // Searchable beyond the visible label: the id (pasted from elsewhere) and
    // the full path, which the row may have shortened to fit.
    filterText: `${s.id} ${s.cwd} ${s.branch} ${s.account || ''}`
  });
  const current = sessions.filter((s) => s.current);
  const others = sessions.filter((s) => !s.current);
  if (current.length) {
    rows.push({ divider: true, label: 'resume · this project' }, ...current.map((s) => toRow(s, false)));
  }
  if (others.length) {
    rows.push({ divider: true, label: 'resume · other projects' }, ...others.map((s) => toRow(s, true)));
  }
  return rows;
}

// A session defaults to the login that owns it, but every stored profile (plus
// the machine's own login) is available as a destination; picking a different
// one is what makes the launcher stage a fork. `profiles` is [{ name, label }]
// — the label already carries whatever state that harness shows for a login.
// Resolves { local, name } for a destination, { manage: true } when the user
// wants the full profile menu instead, or null when cancelled.
export async function chooseResumeProfile({
  session,
  profiles,
  message = 'Choose the profile to resume this session with:',
  localLabel = "This machine's login",
  manageLabel = 'Log in / manage profiles…'
}) {
  const originalName = session.account || null;
  const mark = (isOriginal) => (isOriginal ? '  \x1b[2m(original)\x1b[0m' : '');
  const choices = [
    {
      label: `${localLabel}  \x1b[2mlocal\x1b[0m${mark(originalName === null)}`,
      value: { local: true, name: '' },
      filterText: 'local default this machine'
    },
    ...profiles.map((profile) => ({
      label: `${profile.label}${mark(profile.name === originalName)}`,
      value: { local: false, name: profile.name },
      filterText: profile.name
    })),
    { label: manageLabel, value: { manage: true } }
  ];
  const choice = await select({
    message,
    choices,
    startIndex: originalName === null ? 0 : Math.max(0, choices.findIndex((c) => c.value?.name === originalName)),
    filterable: true
  }).catch(() => null);
  return choice ? choice.value : null;
}

const isWithin = (root, candidate) => {
  const relative = path.relative(path.resolve(root), path.resolve(candidate));
  return Boolean(relative) && !relative.startsWith('..') && !path.isAbsolute(relative);
};

// Temporarily make one login's session files visible inside another. The
// harness reads the copy and writes the continued conversation as a new
// session of its own; cleanup removes only what was created here, never the
// source and never the fork. `entries` is [{ source, target }] with absolute
// paths — missing sources are skipped, an existing target is an error rather
// than something to overwrite.
export function stageFiles({ targetRoot, entries }) {
  const root = path.resolve(targetRoot);
  const created = [];

  const cleanup = () => {
    for (const entry of [...created].reverse()) {
      if (!isWithin(root, entry.path)) continue;
      try {
        if (entry.directory) fs.rmSync(entry.path, { recursive: true, force: true });
        else fs.unlinkSync(entry.path);
      } catch { /* best effort; the original session is still untouched */ }
    }
    // Directories the copy had to create are left behind empty — drop them,
    // walking up to the profile root. A non-empty one throws and stays.
    for (const entry of [...created].reverse()) {
      let dir = path.dirname(path.resolve(entry.path));
      while (isWithin(root, dir)) {
        try { fs.rmdirSync(dir); } catch { break; }
        dir = path.dirname(dir);
      }
    }
  };

  try {
    for (const { source, target } of entries) {
      if (!fs.existsSync(source)) continue;
      if (!isWithin(root, target)) throw new Error('Refusing to copy a session artifact outside the destination profile.');
      if (fs.existsSync(target)) throw new Error(`The destination profile already has session artifact: ${target}`);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      const directory = fs.statSync(source).isDirectory();
      created.push({ path: target, directory });
      if (directory) fs.cpSync(source, target, { recursive: true, force: false, errorOnExist: true });
      else fs.copyFileSync(source, target, fs.constants.COPYFILE_EXCL);
    }
    return { staged: created.map((c) => c.path), cleanup };
  } catch (error) {
    cleanup();
    throw error;
  }
}
