// Guard rails an 8B model needs and Claude Code does not supply. The endpoint runs in bro's own
// process on the same machine as the harness, so it can look at the real files and check a change
// before Claude Code applies it. Every rule below came from a failure in scripts/chatjimmy-eval.js.
//
// A rule either lets the call through (possibly rewritten), replaces it, turns it into an answer,
// or returns a `problem` that is shown to the model so it can try again.
//
//   repeat             the exact call was already made and nothing changed since → use that result
//   echo / answer-cmd  a shell command that only repeats the answer, or hands an invented program a
//                      sentence ("jake 'A closure is…'") → that sentence is the answer
//   bare-interpreter   `node` / `python` with nothing to run
//   invented-filter    Grep narrowed to a file type the request never mentioned → search everything
//   read-window        Read of 1–2 lines at a guessed offset → widen to ~25 lines around it
//   question-only      the user asked a question → no file changes
//   read-before-write  Edit/Write on an existing file not yet read → Read it first
//   not-found / ambiguous  old_string missing, or matching several places → say which lines; when the
//                      request names the function it is in, pick that occurrence automatically
//   lost-code          the change deletes a declaration the request did not ask to remove
//   off-target         the request is about mul(); the change edits add() (or also edits add())
//   append-intent      asked to add code, the model Wrote only the new code → append it instead
//   no-clobber         a Write that throws away most of a file
//   syntax             the file after the change does not parse (JS via `node --check`, JSON)

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const norm = (p) => {
  const s = String(p || '').replace(/\\/g, '/').replace(/\/+$/, '');
  return /^[a-zA-Z]:\//.test(s) ? s.toLowerCase() : s;
};

const readFileSafe = (p) => { try { return fs.statSync(p).isFile() ? fs.readFileSync(p, 'utf8') : null; } catch { return null; } };
const REMINDER_RE = /<system-reminder>[\s\S]*?<\/system-reminder>/g;
const contentBlocks = (m) => (typeof m.content === 'string' ? [{ type: 'text', text: m.content }] : m.content || []);

// ---------------------------------------------------------------- conversation facts

/** Absolute paths the conversation has already read or written (from past tool_use blocks). */
export function filesSeen(messages, cwd) {
  const seen = new Set();
  for (const m of messages || []) {
    if (m.role !== 'assistant' || !Array.isArray(m.content)) continue;
    for (const b of m.content) {
      if (b.type !== 'tool_use' || !['Read', 'Write', 'Edit', 'NotebookEdit'].includes(b.name)) continue;
      const p = b.input?.file_path || b.input?.notebook_path;
      if (p) seen.add(norm(path.isAbsolute(p) || /^[a-zA-Z]:[\\/]/.test(p) || !cwd ? p : path.resolve(cwd, p)));
    }
  }
  return seen;
}

/** Tool calls made since the user's latest request (the task in progress), oldest first. */
export function recentCalls(messages) {
  const out = [];
  for (let i = (messages || []).length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role === 'user') {
      if (contentBlocks(m).some((b) => b.type === 'text' && b.text.replace(REMINDER_RE, '').trim())) break;
    } else if (m.role === 'assistant' && Array.isArray(m.content)) {
      for (const b of [...m.content].reverse()) if (b.type === 'tool_use') out.push({ name: b.name, input: b.input });
    }
  }
  return out.reverse();
}

/** The user's latest request text (reminders removed), or ''. */
export function latestRequest(messages) {
  for (let i = (messages || []).length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.role !== 'user') continue;
    const text = contentBlocks(m).filter((b) => b.type === 'text').map((b) => b.text.replace(REMINDER_RE, '').trim()).filter(Boolean).join('\n');
    if (text) return text;
  }
  return '';
}

// ---------------------------------------------------------------- code facts

const DECL_RE = /\b(?:function\*?|class|const|let|var|def|fn|func)\s+([A-Za-z_$][\w$]*)/g;
const DECL_LINE_RE = /^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:pub\s+)?(?:function\*?|class|const|let|var|def|fn|func)\s+([A-Za-z_$][\w$]*)/;

/** Names declared in a code fragment. */
export const declared = (code) => [...code.matchAll(DECL_RE)].map((m) => m[1]);

/** name → text of its block (from its declaration line to the next declaration). */
function declarationBlocks(code) {
  const lines = code.split('\n');
  const blocks = new Map();
  let name = null, start = 0;
  const close = (end) => { if (name && !blocks.has(name)) blocks.set(name, lines.slice(start, end).join('\n').trimEnd()); };
  lines.forEach((line, i) => {
    const m = line.match(DECL_LINE_RE);
    if (m) { close(i); name = m[1]; start = i; }
  });
  close(lines.length);
  return blocks;
}

/** The declaration enclosing a character offset. */
function enclosingDeclaration(code, offset) {
  const lines = code.slice(0, offset).split('\n');
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = i === lines.length - 1 ? code.split('\n')[i] : lines[i];
    const m = line.match(DECL_LINE_RE);
    if (m) return m[1];
  }
  return null;
}

const occurrences = (hay, needle) => {
  const out = [];
  for (let i = hay.indexOf(needle); i !== -1 && needle; i = hay.indexOf(needle, i + needle.length)) out.push(i);
  return out;
};
const lineOf = (code, offset) => code.slice(0, offset).split('\n').length;
const lineText = (code, n) => code.split('\n')[n - 1];

// Names in the file that the request mentions ("The mul function …" → mul).
const targetsIn = (task, code) => [...new Set(declared(code))].filter((n) => n.length > 1 && new RegExp(`\\b${n.replace(/\$/g, '\\$')}\\b`).test(task));

// Function-like names the request mentions ("sub(a, b)" → sub), declared yet or not.
const codeNames = (task) => [...new Set([...task.matchAll(/\b([A-Za-z_$][\w$]*)\s*\(/g)].map((m) => m[1]))]
  .filter((n) => !/^(if|for|while|switch|return|console|require|import|e\.g|i\.e)$/i.test(n));

/** Why `content` would not parse as `file`, or null. JS is checked with `node --check`, JSON parsed. */
export function syntaxProblem(file, content) {
  const ext = path.extname(file).toLowerCase();
  if (ext === '.json') {
    try { JSON.parse(content); return null; } catch (e) { return e.message; }
  }
  if (!['.js', '.mjs', '.cjs'].includes(ext)) return null;
  const esm = ext === '.mjs' || (ext === '.js' && /^\s*(import|export)\b/m.test(content));
  const tmp = path.join(os.tmpdir(), `bro-cj-check-${process.pid}-${Date.now()}${esm ? '.mjs' : '.cjs'}`);
  try {
    fs.writeFileSync(tmp, content);
    // Node's --check parses without running. Under Bun, execPath is bun, whose --check would run the
    // file — so always ask node, and skip the check if there is none.
    const node = process.versions.bun ? 'node' : process.execPath;
    const r = spawnSync(node, ['--check', tmp], { encoding: 'utf8', timeout: 5000, windowsHide: true });
    if (r.status === 0 || r.error) return null;
    const msg = (r.stderr || '').split('\n').map((l) => l.trim()).filter(Boolean);
    return msg.find((l) => /Error/.test(l)) || msg[0] || 'syntax error';
  } finally {
    try { fs.unlinkSync(tmp); } catch {}
  }
}

// Code cut short inside a JSON string is almost always an unescaped double quote.
const quoteHint = (err) => (/end of input|Unterminated|missing \)/i.test(err)
  ? ' The code was cut short — inside the JSON, a double quote must be written as \\", or use single quotes in the code (console.log(\'hi\')).'
  : '');

/** require() in an ES-module project (package.json "type": "module", or .mjs) cannot run. */
export function moduleStyleProblem(file, content) {
  const ext = path.extname(file).toLowerCase();
  if (!['.js', '.mjs'].includes(ext) || !/\brequire\s*\(/.test(content) || /^\s*import\s/m.test(content)) return null;
  let esm = ext === '.mjs';
  for (let dir = path.dirname(file); !esm; dir = path.dirname(dir)) {
    const pkg = readFileSafe(path.join(dir, 'package.json'));
    if (pkg !== null) { try { esm = JSON.parse(pkg).type === 'module'; } catch {} break; }
    if (path.dirname(dir) === dir) break;
  }
  if (!esm) return null;
  const mod = content.match(/require\(\s*['"]([^'"]+)['"]\s*\)/)?.[1];
  return `This project uses ES modules (package.json has "type": "module"), where require() does not work. ` +
    `Use import instead${mod ? `, e.g. import { add } from '${mod}';` : '.'}`;
}

// ---------------------------------------------------------------- vocabulary

const REMOVING = /\b(remove|delete|rename|replace|drop|rewrite|refactor|swap)\b/i;
// "add" as a verb ("Add a function…", "…and add an export"), not a function called add.
const ADDING_RE = /(^\s*(add|append|insert|include|create|implement|introduce)\b)|\b(add|append|insert|include|create|implement|introduce)\s+(a|an|the|new|another|some|one|two|function|method|class|field|property|test|tests|line|lines|entry|export|helper|route|endpoint)\b/i;
const ADDING = { test: (task) => ADDING_RE.test(task) && !REMOVING.test(task) };
const EVERYWHERE = /\b(all|every|each|everywhere)\b/i;
const CHANGING = /\b(add|append|insert|change|edit|update|modify|fix|set|create|write|make|remove|delete|rename|replace|implement|refactor|move|convert|bump|save|generate)\b/i;
const MUTATING = new Set(['Write', 'Edit', 'NotebookEdit', 'Bash', 'PowerShell']);
const CLAIMS_CHANGE = /\b(has|have|was|were|is now|are now|been)\s+(been\s+)?(updated|changed|created|added|bumped|fixed|renamed|written|modified|set|replaced|removed|deleted|incremented|saved|moved)\b|\b(I|I've|I have)\s+(updated|changed|created|added|bumped|fixed|renamed|wrote|modified|set|replaced|removed|deleted|incremented|saved|moved)\b/i;
const INTERPRETERS = new Set(['node', 'python', 'python3', 'py', 'bash', 'sh', 'zsh', 'pwsh', 'powershell', 'cmd', 'irb', 'deno', 'bun']);

// A request that asks for information and nothing else.
const isQuestionOnly = (task) => {
  const t = task.trim();
  return /\?\s*$/.test(t) && /^(what|which|who|whom|whose|where|when|why|how|is|are|was|were|does|do|did|can|could|should|would|will|has|have|tell|explain|list|show)\b/i.test(t) && !CHANGING.test(t);
};

// Programs a real command line starts with; a sentence handed to anything else is an answer.
const KNOWN_PROGRAMS = new Set(('echo printf git node npm npx pnpm yarn bun deno python python3 py pip pip3 uv cargo go rustc make cmake ' +
  'cat head tail grep rg sed awk sort uniq wc ls dir find mkdir rm rmdir cp mv touch chmod tee cd pwd which where type curl wget ' +
  'gh docker kubectl tsc jest vitest pytest mocha dotnet java javac mvn gradle ruby perl php composer code powershell pwsh cmd ' +
  'write-output write-host set-content add-content get-content out-file new-item select-string').split(/\s+/));

const callKey = (c) => `${c.name}:${JSON.stringify(Object.fromEntries(Object.entries(c.input || {}).map(([k, v]) => [k, typeof v === 'string' ? norm(v) : v]).sort()))}`;

// ---------------------------------------------------------------- answers

/**
 * An answer given right after a Grep/Glob that listed files should name at least one of them
 * (seen: "src/math.js imports from math.js" over a result listing calc.js and report.js, and
 * "These files import from math.js." naming none). Returns a problem string, or null.
 */
export function answerProblem(messages, answer) {
  // claimed-change: "The version has been updated to 2.4.2." with no Edit/Write/command since the
  // request — a change that only happened in the answer (seen on a follow-up turn).
  const task = latestRequest(messages);
  if (task && !isQuestionOnly(task) && CHANGING.test(task) && CLAIMS_CHANGE.test(answer) && !recentCalls(messages).some((c) => MUTATING.has(c.name))) {
    return 'Nothing has been changed yet: no Edit, Write or command has run for this request. Make the change with a tool call now; answer only after it has succeeded.';
  }
  const lastUser = [...(messages || [])].reverse().find((m) => m.role === 'user');
  const results = contentBlocks(lastUser || {}).filter((b) => b.type === 'tool_result' && !b.is_error);
  if (!results.length) return null;
  const callsById = new Map();
  for (const m of messages) if (m.role === 'assistant' && Array.isArray(m.content)) for (const b of m.content) if (b.type === 'tool_use') callsById.set(b.id, b.name);
  const said = answer.toLowerCase();
  const resultText = (r) => (typeof r.content === 'string' ? r.content : (r.content || []).map((x) => x.text || '').join('\n'));

  // A command's output the user asked about ("tell me the output") has to be in the answer
  // (seen: "The script was run successfully." over "hi from jimmy").
  if (/\b(output|print(ed|s)?|say|says|show(ed|s)?|result|returns?|tell me)\b/i.test(latestRequest(messages))) {
    for (const r of results) {
      if (!['Bash', 'PowerShell'].includes(callsById.get(r.tool_use_id))) continue;
      const out = resultText(r).trim();
      if (!out || out.length > 300 || /^\(.*no output\)$/i.test(out)) continue;
      const words = out.toLowerCase().match(/[a-z0-9][\w.-]{1,}/g) || [];
      if (words.length && !words.some((w) => said.includes(w))) {
        return `Tell the user what the command printed. Its output was:\n${out}`;
      }
    }
  }

  const files = [];
  for (const r of results) {
    if (!['Grep', 'Glob'].includes(callsById.get(r.tool_use_id))) continue;
    const text = typeof r.content === 'string' ? r.content : (r.content || []).map((x) => x.text || '').join('\n');
    for (const line of text.split('\n')) {
      const f = line.trim().replace(/:\d+(:.*)?$/, '');
      if (/[\\/]?[\w.-]+\.\w{1,8}$/.test(f) && !/\s/.test(f)) files.push(path.basename(f.replace(/\\/g, '/')));
    }
  }
  if (!files.length || files.length > 30) return null;
  if (files.some((f) => said.includes(f.toLowerCase()))) return null;
  return `Your answer does not name any of the files the last search found (${[...new Set(files)].slice(0, 8).join(', ')}). ` +
    'Answer the user again, using exactly what that result shows.';
}

// ---------------------------------------------------------------- the policy

/**
 * Check a repaired call against the conversation and the disk.
 * @returns {{ call: {name, input} | null, problem?: string, answer?: string, note?: string }}
 *   call     the call to send (possibly rewritten, e.g. into a Read); null to drop it
 *   problem  send this back to the model instead of making the call
 *   answer   the call was really a text answer; reply with this
 */
export function applyPolicy(call, { messages, cwd, tools, text = '' }) {
  if (!call) return { call };
  const names = new Set(tools.map((t) => t.name));
  const task = latestRequest(messages);

  // repeat
  const key = callKey(call);
  const recent = recentCalls(messages);
  const last = recent.map(callKey).lastIndexOf(key);
  if (last !== -1 && !recent.slice(last + 1).some((c) => MUTATING.has(c.name))) {
    return { call: null, problem: `You already made exactly this ${call.name} call and its result is above. Use that result to answer the user, or make a different call.` };
  }

  if (call.name === 'Bash' || call.name === 'PowerShell') {
    const cmd = String(call.input.command || '').trim();
    const probe = text.replace(/\s+/g, ' ').slice(0, 40);
    if (text.length >= 120 && probe.length >= 30 && cmd.replace(/\s+/g, ' ').includes(probe)) {
      return { call: null, note: 'dropped a command that only repeated the answer' };
    }
    const program = cmd.split(/\s+/)[0].replace(/^.*[\\/]/, '').replace(/\.exe$/i, '').toLowerCase();
    const quoted = cmd.match(/(['"])((?:(?!\1)[\s\S]){40,})\1/)?.[2];
    if (quoted && !KNOWN_PROGRAMS.has(program) && quoted.trim().split(/\s+/).length >= 8 && /[a-z]{3}/i.test(quoted)) {
      return { call: null, answer: quoted.trim(), note: `took the answer out of a "${program}" command` };
    }
    if (INTERPRETERS.has(program) && cmd.split(/\s+/).length === 1) {
      return { call: null, problem: `Running "${cmd}" on its own does nothing here. To see what is in a file, Read it; to run a script, give its path.` };
    }
  }

  // search paths: a path that does not exist (the model passes the import target, "./math.js", or a
  // glob, "./**") fails with "Path does not exist"; a first search narrowed to a folder the request
  // never mentioned misses the answer (the phrase was in README.md, the search was in src/).
  if ((call.name === 'Grep' || call.name === 'Glob') && typeof call.input.path === 'string') {
    const p = call.input.path;
    const exists = (() => { try { fs.statSync(p); return true; } catch { return false; } })();
    const rel = cwd ? path.relative(cwd, p).replace(/\\/g, '/') : path.basename(p);
    const mentioned = rel && task && (task.includes(rel) || task.includes(path.basename(p)));
    const firstSearch = !recent.some((c) => c.name === call.name);
    if (!exists || (call.name === 'Grep' && firstSearch && !mentioned)) {
      const { path: _dropped, ...rest } = call.input;
      call = { ...call, input: rest };
    }
  }

  if (call.name === 'Grep' && typeof call.input.glob === 'string' && task) {
    const ext = call.input.glob.match(/\.([\w]+)\}?$/)?.[1];
    if (ext && !new RegExp(`\\.?\\b${ext}\\b`, 'i').test(task)) {
      const { glob, ...rest } = call.input;
      call = { ...call, input: rest };
    }
  }

  // read-window: a 1–2 line Read. From the top of the file it means "read the file" (the focused view
  // then finds what the request names); further in, it is a guess at a line, so show ~25 around it.
  if (call.name === 'Read' && Number.isFinite(call.input.limit) && call.input.limit < 20) {
    const { offset: o, limit: _l, ...rest } = call.input;
    call = (Number(o) || 0) <= 1
      ? { ...call, input: rest }
      : { ...call, input: { ...rest, offset: Math.max(1, Number(o) - 3), limit: 25 } };
  }

  if (['Edit', 'Write', 'NotebookEdit'].includes(call.name) && task && isQuestionOnly(task)) {
    return { call: null, problem: 'The user only asked a question. Do not change any files — answer the question from what you have found.' };
  }

  if (!((call.name === 'Edit' || call.name === 'Write') && typeof call.input.file_path === 'string')) return { call };

  const file = call.input.file_path;
  const base = path.basename(file);
  const existing = readFileSafe(file);

  if (existing === null) {
    if (call.name === 'Write' && typeof call.input.content === 'string') {
      const bad = syntaxProblem(file, call.input.content);
      if (bad) return { call: null, problem: `That ${base} would not parse: ${bad}. Write valid code.${quoteHint(bad)}` };
      const esm = moduleStyleProblem(file, call.input.content);
      if (esm) return { call: null, problem: esm };
    }
    return { call };
  }

  if (!filesSeen(messages, cwd).has(norm(file)) && names.has('Read')) {
    return { call: { name: 'Read', input: { file_path: file } }, note: `read ${base} before changing it` };
  }

  let after;
  let notes = [];
  if (call.name === 'Edit') {
    let { old_string: oldS, new_string: newS } = call.input;
    if (typeof oldS !== 'string' || typeof newS !== 'string') return { call };
    // append-intent (Edit form): asked to add X, the model "adds" it by overwriting existing code
    // ("function add(a, b)" → "function sub(a, b) {…}"). Add the new declaration at the end instead.
    const newNames = declared(newS).filter((n) => !new RegExp(`\\b${n}\\b`).test(existing));
    const wanted = codeNames(task);
    if (ADDING.test(task) && newNames.length && newNames.some((n) => !wanted.length || wanted.includes(n))) {
      const trial = existing.includes(oldS) ? existing.replace(oldS, () => newS) : null;
      const damages = trial === null || syntaxProblem(file, trial) ||
        [...new Set(declared(existing))].some((n) => !declared(trial).includes(n));
      if (damages) {
        const start = newS.search(new RegExp(`(?:export\\s+)?(?:async\\s+)?(?:function\\*?|class|const|let|var|def)\\s+${newNames[0]}\\b`));
        let code = newS.slice(Math.max(0, start)).trim();
        if (!/^export\b/.test(code) && /^\s*export\s/m.test(existing) && !/^\s*(?!export)(function|class|const)\s/m.test(existing)) code = `export ${code}`;
        const lastLine = existing.replace(/\s+$/, '').split('\n').at(-1);
        call = { name: 'Edit', input: { file_path: file, old_string: lastLine, new_string: `${lastLine}\n${code}` } };
        ({ old_string: oldS, new_string: newS } = call.input);
        notes.push(`turned an overwriting Edit into an append of ${newNames[0]}() to ${base}`);
      }
    }
    const hits = occurrences(existing, oldS);
    if (!hits.length) {
      return { call: null, problem: `old_string was not found in ${base}. Copy it exactly from the file as Read showed it (without the line numbers).` };
    }
    if (hits.length > 1 && !call.input.replace_all) {
      // When the request names the function one occurrence sits in, that is the one meant.
      const targets = targetsIn(task, existing);
      const inTarget = hits.filter((h) => targets.includes(enclosingDeclaration(existing, h)));
      if (inTarget.length === 1) {
        const n = lineOf(existing, inTarget[0]);
        const line = lineText(existing, n);
        if (occurrences(existing, line).length === 1) {
          const wider = { ...call.input, old_string: line, new_string: line.replace(oldS, () => newS) };
          call = { ...call, input: wider };
          ({ old_string: oldS, new_string: newS } = wider);
          notes.push(`narrowed an ambiguous Edit to line ${n} (${enclosingDeclaration(existing, inTarget[0])})`);
        }
      } else {
        const lines = hits.map((h) => lineOf(existing, h));
        return {
          call: null,
          problem: `old_string appears ${hits.length} times in ${base} (lines ${lines.join(', ')}). Use the whole line you mean as old_string, ` +
            `e.g. line ${lines.at(-1)}: ${JSON.stringify(lineText(existing, lines.at(-1)).trim())}.`
        };
      }
    }
    after = call.input.replace_all ? existing.split(oldS).join(newS) : existing.replace(oldS, () => newS);
  } else {
    const content = String(call.input.content ?? '');
    const before = existing.split('\n').filter((l) => l.trim()).length;
    const count = content.split('\n').filter((l) => l.trim()).length;
    const fresh = declared(content);
    if (count < before && ADDING.test(task) && fresh.length && fresh.every((n) => !new RegExp(`\\b${n}\\b`).test(existing))) {
      const lastLine = existing.replace(/\s+$/, '').split('\n').at(-1);
      call = { name: 'Edit', input: { file_path: file, old_string: lastLine, new_string: `${lastLine}\n${content.replace(/\s+$/, '')}` } };
      after = existing.replace(lastLine, () => call.input.new_string);
      notes.push(`turned a clobbering Write into an append to ${base}`);
    } else {
      if (before >= 2 && count < before * 0.6) {
        return {
          call: null,
          problem: `That Write would replace all ${before} lines of ${base} with ${count}, losing the rest of the file. ` +
            'Use Edit to change only the part that needs changing (old_string = the exact current text, new_string = the new text), or Write the complete new file.'
        };
      }
      after = content;
    }
  }

  // Unparseable code is the most basic problem and the clearest one to report.
  const bad = syntaxProblem(file, after);
  if (bad) return { call: null, problem: `After that change ${base} would not parse: ${bad}. Fix the ${call.name === 'Edit' ? 'new_string' : 'content'} so the file stays valid.${quoteHint(bad)}` };
  const esm = moduleStyleProblem(file, after);
  if (esm) return { call: null, problem: esm };

  if (!REMOVING.test(task)) {
    const kept = new Set(declared(after));
    const lost = [...new Set(declared(existing))].filter((n) => !kept.has(n));
    if (lost.length) {
      return {
        call: null,
        problem: `That change deletes ${lost.map((n) => `${n}()`).join(', ')}, which the request did not ask to remove. ` +
          'To add code, keep the existing text: set new_string to the old_string followed by the new code.'
      };
    }
  }

  // off-target: which declarations changed, against the ones the request is about.
  const targets = [...new Set([...targetsIn(task, existing), ...codeNames(task)])];
  if (targets.length) {
    const was = declarationBlocks(existing), now = declarationBlocks(after);
    const changed = [...new Set([...was.keys(), ...now.keys()])].filter((n) => was.get(n) !== now.get(n));
    const onTarget = changed.filter((n) => targets.includes(n));
    const collateral = changed.filter((n) => !targets.includes(n) && was.has(n));
    if (changed.length && !onTarget.length) {
      // For an addition, spell out an edit that can be copied: append after the file's real last line.
      const missing = targets.filter((n) => !declared(existing).includes(n));
      const lastLine = existing.replace(/\s+$/, '').split('\n').at(-1);
      const how = ADDING.test(task) && missing.length
        ? ` To add ${missing[0]}() at the end of ${base}, use Edit with old_string ${JSON.stringify(lastLine)} and new_string set to that same line, a newline, then the complete new ${missing[0]}() code.`
        : ' Make the change the user asked for.';
      return { call: null, problem: `The request is about ${targets.map((n) => `${n}()`).join(', ')}, but that change edits ${changed.map((n) => `${n}()`).join(', ')}.${how}` };
    }
    if (collateral.length && !EVERYWHERE.test(task)) {
      return { call: null, problem: `That change also edits ${collateral.map((n) => `${n}()`).join(', ')}, which the request did not mention. Change only ${onTarget.map((n) => `${n}()`).join(', ')}.` };
    }
  }

  return { call, note: notes.join('; ') || undefined };
}
