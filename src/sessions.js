// Claude Code session discovery.
//
// Claude Code writes one JSONL transcript per session under
// <config dir>/projects/<encoded cwd>/<session id>.jsonl, where the directory
// name is the working directory with every non-alphanumeric character replaced
// by '-'. That encoding is lossy (F--bro-cli could be F:\bro-cli or F:/bro/cli),
// so the real cwd is read back out of the transcript itself — once per project
// directory, since every session in one shares the same cwd.
//
// bro's account profiles each have their own config dir, so each profile has its
// own set of sessions; a session can only be resumed by the profile that owns it.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { BRO_DIR } from './config.js';
import { fit } from './ui.js';

const CACHE_PATH = path.join(BRO_DIR, 'sessions.cache.json');

// How much of a transcript's head to read when looking for its first prompt.
// Prompts sit within the first few entries; the cap keeps a 100MB transcript
// from being pulled into memory.
const HEAD_BYTES = 128 * 1024;
// Newest-first cap on how many sessions are described (and so filterable).
// A scan of everything on a well-used machine is thousands of files.
const MAX_SESSIONS = 400;
// Files opened at once while filling the cache.
const CONCURRENCY = 24;

// Wrapper text Claude Code injects around slash commands, hook output and
// image attachments. None of it is the user's prompt, so a transcript that
// opens with it is searched further for the real thing.
const NOISE_PREFIX = [
  '<local-command-caveat>',
  '<command-name>',
  '<command-message>',
  '<local-command-stdout>',
  '<user-prompt-submit-hook>',
  '<system-reminder>',
  '<bash-input>',
  '<bash-stdout>',
  'Caveat: The messages below',
  // A slash command that loads a skill injects the skill's own text as the
  // next user message; the command name above it is the real title.
  'Base directory for this skill:'
];

export function encodeProjectDir(cwd) {
  return String(cwd || '').replace(/[^a-zA-Z0-9]/g, '-');
}

const samePath = (a, b) =>
  Boolean(a) && Boolean(b) &&
  (process.platform === 'win32'
    ? path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase()
    : path.resolve(a) === path.resolve(b));

// Read at most `bytes` from the head of a file without loading the whole thing.
function readHead(file, bytes = HEAD_BYTES) {
  let fd = null;
  try {
    fd = fs.openSync(file, 'r');
    const buf = Buffer.allocUnsafe(bytes);
    const read = fs.readSync(fd, buf, 0, bytes, 0);
    return buf.toString('utf8', 0, read);
  } catch {
    return '';
  } finally {
    if (fd !== null) try { fs.closeSync(fd); } catch {}
  }
}

// Message content is either a plain string or a block array; only text blocks
// carry a prompt (tool results and images are not what the user typed).
function textOf(content) {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return content
    .filter((b) => b && b.type === 'text' && typeof b.text === 'string')
    .map((b) => b.text)
    .join(' ');
}

// Strip the "[Image: …]" preambles Claude Code prepends to pasted screenshots so
// a prompt that came with one still shows its words instead of image geometry.
const clean = (s) =>
  String(s || '')
    .replace(/\[Image:[^\]]*\]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();

const isNoise = (s) => NOISE_PREFIX.some((p) => s.startsWith(p));

// Sessions opened by a slash command carry no typed prompt at all. The command
// is what the user chose, so it names the session rather than hiding it.
function commandTitle(s) {
  const name = /<command-name>\s*(\/?[^<\s]+)/.exec(s);
  if (!name) return '';
  const args = /<command-args>\s*([^<]*)/.exec(s);
  return `${name[1].startsWith('/') ? '' : '/'}${name[1]}${args && args[1].trim() ? ' ' + args[1].trim() : ''}`;
}

// Long prompts are truncated everywhere they're shown; storing the whole thing
// only bloats the cache.
const MAX_TITLE = 200;

// Pull { title, cwd, branch } out of a transcript's opening entries. Returns
// title '' when the session holds no real prompt — those are abandoned starts
// with nothing to resume, and callers drop them.
export function describeTranscript(text) {
  let title = '';
  let command = '';
  let cwd = '';
  let branch = '';
  for (const line of String(text).split('\n')) {
    if (!line || line[0] !== '{') continue;
    let entry;
    try {
      entry = JSON.parse(line);
    } catch {
      // The head cut this line mid-object. A prompt that opens with a pasted
      // image is megabytes of base64, so that entry is *always* truncated —
      // recover what can be read rather than losing the session.
      if (!title && /"type":"user"/.test(line)) {
        const match = /"type":"text","text":"((?:[^"\\]|\\.){2,300})/.exec(line);
        // The capture can still end inside a \uXXXX escape; an unreadable
        // snippet is no worse than the one we already had.
        try {
          const snippet = match ? clean(JSON.parse(`"${match[1]}"`)) : '';
          if (snippet && !isNoise(snippet)) title = snippet.slice(0, MAX_TITLE);
        } catch { /* fall through to the image fallback */ }
        if (!title && /"type":"image"/.test(line)) command ||= '(image)';
      }
      continue;
    }
    if (!cwd && typeof entry.cwd === 'string') cwd = entry.cwd;
    if (!branch && typeof entry.gitBranch === 'string') branch = entry.gitBranch;
    // A compacted session records its own summary — better than any prompt.
    if (entry.type === 'summary' && typeof entry.summary === 'string' && entry.summary.trim()) {
      title = clean(entry.summary).slice(0, MAX_TITLE);
      if (cwd) break;
      continue;
    }
    if (title || entry.type !== 'user' || entry.isMeta || entry.isSidechain) continue;
    const content = entry.message?.content;
    const raw = textOf(content);
    const body = clean(raw);
    if (!body) {
      // A wordless screenshot is still a session worth listing.
      if (Array.isArray(content) && content.some((b) => b?.type === 'image')) command ||= '(image)';
      continue;
    }
    if (isNoise(body)) command ||= commandTitle(raw);
    else title = body.slice(0, MAX_TITLE);
  }
  return { title: title || command, cwd, branch };
}

function loadCache() {
  try {
    const cache = JSON.parse(fs.readFileSync(CACHE_PATH, 'utf8'));
    return cache && typeof cache === 'object' ? cache : {};
  } catch {
    return {};
  }
}

function saveCache(cache) {
  try {
    fs.mkdirSync(BRO_DIR, { recursive: true });
    fs.writeFileSync(CACHE_PATH, JSON.stringify(cache));
  } catch {
    /* best-effort — a missing cache only costs a rescan */
  }
}

// Every .jsonl transcript under one config dir, with its stat data. Cheap:
// directory reads and stats only, no file contents.
function statSessions(configDir, account) {
  const root = path.join(configDir, 'projects');
  let dirs = [];
  try {
    dirs = fs.readdirSync(root, { withFileTypes: true }).filter((d) => d.isDirectory());
  } catch {
    return [];
  }
  const out = [];
  for (const dir of dirs) {
    const projectDir = path.join(root, dir.name);
    let files = [];
    try {
      files = fs.readdirSync(projectDir);
    } catch {
      continue;
    }
    for (const name of files) {
      if (!name.endsWith('.jsonl')) continue;
      const file = path.join(projectDir, name);
      let stat;
      try {
        stat = fs.statSync(file);
      } catch {
        continue;
      }
      if (!stat.isFile() || stat.size < 512) continue; // sub-512B files hold no prompt
      out.push({
        id: name.slice(0, -6),
        file,
        account,
        projectKey: dir.name,
        mtime: stat.mtimeMs,
        size: stat.size
      });
    }
  }
  return out;
}

async function mapLimited(items, limit, fn) {
  const out = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i], i);
    }
  };
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
  return out;
}

// List resumable sessions across the given config dirs, newest first.
//
// `sources` is [{ account, configDir }] — `account` is the profile name a
// session must be resumed under (null for the default Claude config dir).
// Each result: { id, file, account, cwd, title, branch, mtime, current }.
// `current` marks sessions belonging to `cwd` (default: process.cwd()).
export async function listSessions({ sources = [], cwd = process.cwd(), limit = MAX_SESSIONS } = {}) {
  const all = [];
  for (const src of sources) all.push(...statSessions(src.configDir, src.account));
  all.sort((a, b) => b.mtime - a.mtime);

  const currentKey = encodeProjectDir(cwd);
  // Keep every session for this project even when it falls outside the newest
  // `limit` — the current project is the list users reach for most.
  const head = all.slice(0, limit);
  const kept = [...head, ...all.slice(limit).filter((s) => s.projectKey === currentKey)];

  const cache = loadCache();
  let dirty = false;
  const described = await mapLimited(kept, CONCURRENCY, async (s) => {
    const hit = cache[s.file];
    if (hit && hit.mtime === s.mtime && hit.size === s.size) {
      return { ...s, title: hit.title, cwd: hit.cwd, branch: hit.branch };
    }
    const info = describeTranscript(readHead(s.file));
    cache[s.file] = { mtime: s.mtime, size: s.size, ...info };
    dirty = true;
    return { ...s, ...info };
  });

  if (dirty) {
    // Drop entries for transcripts that no longer exist so the cache tracks
    // the on-disk set instead of growing forever.
    const live = new Set(kept.map((s) => s.file));
    for (const key of Object.keys(cache)) if (!live.has(key)) delete cache[key];
    saveCache(cache);
  }

  // Sessions in the same project directory share a cwd, so one transcript that
  // recorded it covers the rest (older transcripts predate the cwd field).
  const cwdByProject = new Map();
  for (const s of described) {
    if (s.cwd && !cwdByProject.has(s.projectKey)) cwdByProject.set(s.projectKey, s.cwd);
  }

  return described
    .filter((s) => s.title)
    .map((s) => {
      const resolved = s.cwd || cwdByProject.get(s.projectKey) || '';
      return {
        id: s.id,
        file: s.file,
        account: s.account,
        cwd: resolved,
        title: s.title,
        branch: s.branch || '',
        mtime: s.mtime,
        current: s.projectKey === currentKey || samePath(resolved, cwd)
      };
    });
}

// Home-relative, tail-biased path for a list row: the end of a path says which
// project it is, so an over-long one loses its head rather than its name.
export function shortPath(p, max = 34) {
  let s = String(p || '');
  const home = os.homedir();
  if (home && s.toLowerCase().startsWith(home.toLowerCase())) s = '~' + s.slice(home.length);
  return s.length > max ? '…' + s.slice(s.length - max + 1) : s;
}

// One session row: age, prompt, then dim metadata (path when the session is
// from another project, git branch, and the profile that can resume it).
// The prompt gets a fixed width so the metadata lines up down the column.
export function sessionLabel(session, { showPath = false, titleWidth = 42 } = {}) {
  const age = relativeTime(session.mtime).padStart(3);
  const meta = [
    showPath ? shortPath(session.cwd) : null,
    session.branch || null,
    // No profile means the machine's own Claude login, not a pooled account.
    session.account || 'local'
  ].filter(Boolean).join(' · ');
  return `\x1b[2m${age}\x1b[0m ${fit(session.title, titleWidth)} \x1b[2m${meta}\x1b[0m`;
}

// Compact age for a list row: "now", "3m", "5h", "2d", "6w".
export function relativeTime(ms, now = Date.now()) {
  const secs = Math.max(0, Math.round((now - ms) / 1000));
  if (secs < 60) return 'now';
  const mins = Math.round(secs / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d`;
  const weeks = Math.round(days / 7);
  if (weeks < 52) return `${weeks}w`;
  return `${Math.round(weeks / 52)}y`;
}
