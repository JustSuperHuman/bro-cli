// Codex CLI session discovery.
//
// The codex CLI writes one JSONL "rollout" per session under
// $CODEX_HOME/sessions/<year>/<month>/<day>/rollout-<timestamp>-<uuid>.jsonl.
// Its first line is a session_meta entry carrying the session id, the working
// directory and the git branch, so — unlike Claude Code's transcripts — nothing
// about the project has to be recovered from an encoded directory name.
//
// Each login profile is its own CODEX_HOME with its own sessions/ tree, so a
// listing spans several of them and every row remembers the profile that owns
// it — codex finds a session by scanning the home it runs in, so an owner is
// what says which home that has to be. Rollouts written by sub-agents and by
// non-interactive runs (`codex exec`) are skipped: codex's own picker hides
// them too, and neither is a conversation you can pick up.

import fs from 'node:fs';
import path from 'node:path';
import { BRO_DIR } from './config.js';
import { clean, commandTitle, loadCache, mapLimited, MAX_TITLE, readHead, samePath, saveCache } from './sessions.js';

const CACHE_PATH = path.join(BRO_DIR, 'codex-sessions.cache.json');

// Codex opens a rollout with its complete system prompt (tens of KB) followed
// by the project's AGENTS.md, so its head has to be read far wider than a
// Claude transcript's before the first typed prompt shows up.
const HEAD_BYTES = 512 * 1024;
// Newest-first cap on how many sessions are described (and so filterable).
const MAX_SESSIONS = 400;
// Files opened at once while filling the cache.
const CONCURRENCY = 24;
// sessions/<year>/<month>/<day>/<file> — deep enough for the date layout, and
// a bound on how far a stray directory can send the walk.
const MAX_DEPTH = 4;

// Text Codex injects around the conversation: project instructions, the
// environment preamble, plugin and skill payloads, and slash-command wrappers.
// None of it is what the user typed, so a rollout that opens with it is
// searched further for the real prompt.
const NOISE_PREFIX = [
  '# AGENTS.md instructions',
  '<environment_context',
  '<user_instructions',
  '<codex_internal_context',
  '<recommended_plugins',
  '<plugin',
  '<skill>',
  '<command-name>',
  '<command-message>',
  '<system-reminder',
  '<INSTRUCTIONS>'
];

const isNoise = (s) => NOISE_PREFIX.some((p) => s.startsWith(p));

// Codex marks pasted screenshots inline ("<image name=[Image #1] path=…>"),
// and some versions close the tag as well — neither half is the prompt.
const stripImages = (s) => String(s || '').replace(/<\/?image\b[^>]*>/gi, ' ');
const hasImage = (s) => /<\/?image\b[^>]*>/i.test(String(s || ''));

// A rollout's user-visible text: message content is a block array, of which
// only the text blocks were typed (tool output and images were not).
function textOf(content) {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return content
    .filter((b) => b && (b.type === 'input_text' || b.type === 'text') && typeof b.text === 'string')
    .map((b) => b.text)
    .join(' ');
}

// Sub-agent threads and `codex exec` runs share the sessions directory with
// real conversations but are not resumable work — `source` is an object
// describing the spawn for the former, and the originator names the latter.
function isInteractive(meta) {
  if (!meta) return false;
  if (meta.thread_source === 'subagent') return false;
  if (meta.source && typeof meta.source === 'object') return false;
  return meta.originator !== 'codex_exec' && meta.source !== 'exec';
}

// Pull { id, title, cwd, branch, interactive } out of a rollout's opening
// entries. Returns title '' when the rollout holds no typed prompt — those are
// abandoned starts with nothing to resume, and callers drop them.
export function describeRollout(text) {
  let title = '';
  let command = '';
  let meta = null;
  for (const line of String(text).split('\n')) {
    if (!line || line[0] !== '{') continue;
    let entry;
    try {
      entry = JSON.parse(line);
    } catch {
      // The head cut this line mid-object. A prompt that arrived with a pasted
      // image is megabytes of base64, so that entry is *always* truncated —
      // recover what can be read rather than losing the session.
      if (!title && /"role":"user"/.test(line)) {
        const match = /"type":"input_text","text":"((?:[^"\\]|\\.){2,300})/.exec(line);
        try {
          const snippet = match ? clean(stripImages(JSON.parse(`"${match[1]}"`))) : '';
          if (snippet && !isNoise(snippet)) title = snippet.slice(0, MAX_TITLE);
        } catch { /* fall through to the image fallback */ }
        if (!title && /"type":"input_image"/.test(line)) command ||= '(image)';
      }
      continue;
    }
    const payload = entry.payload || {};
    if (!meta && entry.type === 'session_meta') meta = payload;
    // A rollout is one conversation, so the id/cwd/branch never change — once
    // the first prompt is in hand there is nothing left to read.
    if (title && meta) break;
    if (title) continue;

    const raw =
      payload.type === 'message' && payload.role === 'user'
        ? textOf(payload.content)
        : entry.type === 'event_msg' && payload.type === 'user_message'
          ? String(payload.message || '')
          : '';
    if (!raw) continue;
    const body = clean(stripImages(raw));
    if (!body) {
      // A wordless screenshot is still a session worth listing.
      if (hasImage(raw)) command ||= '(image)';
      continue;
    }
    if (isNoise(body)) command ||= commandTitle(raw);
    else title = body.slice(0, MAX_TITLE);
  }
  return {
    id: String(meta?.session_id || meta?.id || ''),
    title: title || command,
    cwd: typeof meta?.cwd === 'string' ? meta.cwd : '',
    branch: typeof meta?.git?.branch === 'string' ? meta.git.branch : '',
    interactive: isInteractive(meta)
  };
}

// Every rollout under one profile's sessions root, with its stat data. Cheap:
// directory reads and stats only, no file contents.
function statRollouts(root, account, depth = 0) {
  let entries = [];
  try {
    entries = fs.readdirSync(root, { withFileTypes: true });
  } catch {
    return [];
  }
  const out = [];
  for (const entry of entries) {
    const file = path.join(root, entry.name);
    if (entry.isDirectory()) {
      if (depth < MAX_DEPTH) out.push(...statRollouts(file, account, depth + 1));
      continue;
    }
    if (!entry.name.endsWith('.jsonl')) continue;
    let stat;
    try {
      stat = fs.statSync(file);
    } catch {
      continue;
    }
    if (!stat.isFile() || stat.size < 512) continue; // sub-512B files hold no prompt
    out.push({ file, account, mtime: stat.mtimeMs, size: stat.size });
  }
  return out;
}

// List resumable Codex sessions across the given login profiles, newest first.
//
// `sources` is [{ account, home }] — `account` is the profile name the session
// belongs to (null for the machine's own Codex login). Each result:
// { id, file, account, cwd, title, branch, mtime, current }, where `current`
// marks sessions belonging to `cwd` (default: process.cwd()).
export async function listCodexSessions({ sources = [], cwd = process.cwd(), limit = MAX_SESSIONS } = {}) {
  const all = [];
  for (const src of sources) all.push(...statRollouts(path.join(src.home, 'sessions'), src.account ?? null));
  all.sort((a, b) => b.mtime - a.mtime);

  const cache = loadCache(CACHE_PATH);
  // Sessions for this project are the list users reach for most, so any that
  // fall outside the newest `limit` are kept as well — cheaply, since a cached
  // description already knows the directory a rollout belongs to.
  const kept = [
    ...all.slice(0, limit),
    ...all.slice(limit).filter((s) => samePath(cache[s.file]?.cwd, cwd))
  ];

  let dirty = false;
  const described = await mapLimited(kept, CONCURRENCY, async (s) => {
    const hit = cache[s.file];
    if (hit && hit.mtime === s.mtime && hit.size === s.size) return { ...s, ...hit };
    const info = describeRollout(readHead(s.file, HEAD_BYTES));
    cache[s.file] = { mtime: s.mtime, size: s.size, ...info };
    dirty = true;
    return { ...s, ...info };
  });

  if (dirty) {
    // Drop entries for rollouts that no longer exist so the cache tracks the
    // on-disk set instead of growing forever.
    const live = new Set(kept.map((s) => s.file));
    for (const key of Object.keys(cache)) if (!live.has(key)) delete cache[key];
    saveCache(cache, CACHE_PATH);
  }

  return described
    .filter((s) => s.interactive && s.id && s.title)
    .map((s) => ({
      id: s.id,
      file: s.file,
      account: s.account ?? null,
      cwd: s.cwd || '',
      title: s.title,
      branch: s.branch || '',
      mtime: s.mtime,
      current: samePath(s.cwd, cwd)
    }));
}
