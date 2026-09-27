// Tool calling for a model that has none: ChatJimmy is told to answer a tool call with one JSON
// object in Llama 3.1's own custom-tool format — {"name": …, "parameters": {…}} — and this module
// turns that text back into Anthropic tool_use blocks.
//
// Claude Code's tool descriptions run to kilobytes each (the six core tools are ~10k characters),
// far beyond ChatJimmy's ~6k-token window, so each known tool gets a one-line hand-written summary.
// Unknown tools fall back to a line generated from their JSON schema.

import path from 'node:path';

// One line per tool: its argument shape and when to use it. Argument names match Claude Code's schemas.
const KNOWN = {
  Read: '{"file_path": "<path>"} — show a file with line numbers. Optional: "offset", "limit" (line numbers).',
  Write: '{"file_path": "<path>", "content": "<full file text>"} — create a new file. For a file that exists, use Edit.',
  Edit: '{"file_path": "<path>", "old_string": "<exact text now in the file>", "new_string": "<replacement>"} — change part of a file. Read the file first; old_string must match exactly.',
  Bash: '{"command": "<shell command>"} — run a program (node, npm, git, python, tests). Not for reading, finding or editing files.',
  PowerShell: '{"command": "<PowerShell command>"} — run a PowerShell command.',
  Glob: '{"pattern": "<glob like **/*.js>"} — list files whose path matches. Optional: "path" (folder).',
  Grep: '{"pattern": "<regex>"} — search file contents; lists matching files. Optional: "path", "glob" (e.g. "*.ts"), "output_mode": "content" to show lines.'
};

// Names the model reaches for instead of the real ones.
const TOOL_ALIASES = {
  bash: 'Bash', shell: 'Bash', sh: 'Bash', run: 'Bash', run_command: 'Bash', execute: 'Bash', exec: 'Bash', terminal: 'Bash', cmd: 'Bash', command: 'Bash',
  read: 'Read', read_file: 'Read', readfile: 'Read', cat: 'Read', open: 'Read', view: 'Read', open_file: 'Read',
  write: 'Write', write_file: 'Write', writefile: 'Write', create_file: 'Write', save_file: 'Write', create: 'Write',
  edit: 'Edit', edit_file: 'Edit', replace: 'Edit', str_replace: 'Edit', modify_file: 'Edit', update_file: 'Edit',
  glob: 'Glob', find: 'Glob', find_files: 'Glob', list_files: 'Glob', search_files: 'Glob',
  grep: 'Grep', search: 'Grep', search_code: 'Grep', ripgrep: 'Grep', rg: 'Grep',
  powershell: 'PowerShell', pwsh: 'PowerShell'
};

// Argument names the model reaches for instead of the schema's.
const PARAM_ALIASES = {
  file_path: ['path', 'file', 'filename', 'file_name', 'filepath', 'target_file', 'target'],
  content: ['text', 'contents', 'data', 'body', 'file_content', 'code'],
  old_string: ['old', 'old_text', 'oldText', 'search', 'find', 'from', 'original'],
  new_string: ['new', 'new_text', 'newText', 'replace', 'replacement', 'to', 'updated'],
  command: ['cmd', 'script', 'shell', 'code', 'bash'],
  pattern: ['query', 'regex', 'search', 'glob_pattern', 'expression', 'term']
};

// Enum values the model invents, mapped to real ones (Grep's output_mode above all).
const ENUM_ALIASES = {
  list: ['files_with_matches'], files: ['files_with_matches'], filenames: ['files_with_matches'], names: ['files_with_matches'],
  lines: ['content'], text: ['content'], matches: ['content'], show: ['content'],
  number: ['count'], total: ['count']
};

// Parameters that hold a path Claude Code insists be absolute.
const ABSOLUTE_PATH_PARAMS = { Read: ['file_path'], Write: ['file_path'], Edit: ['file_path'], NotebookEdit: ['notebook_path'] };
const OPTIONAL_PATH_PARAMS = { Glob: ['path'], Grep: ['path'] };

const firstSentence = (s = '') => {
  const t = s.replace(/\s+/g, ' ').trim();
  const m = t.match(/^(.{8,160}?[.!?])(\s|$)/);
  return (m ? m[1] : t.slice(0, 140)).trim();
};

export function describeTool(tool) {
  if (KNOWN[tool.name]) return `- ${tool.name} ${KNOWN[tool.name]}`;
  const props = tool.input_schema?.properties || {};
  const required = new Set(tool.input_schema?.required || []);
  const args = Object.keys(props).filter((k) => required.has(k)).map((k) => `"${k}": <${props[k].type || 'value'}>`).join(', ');
  const optional = Object.keys(props).filter((k) => !required.has(k));
  return `- ${tool.name} {${args}} — ${firstSentence(tool.description)}${optional.length ? ` Optional: ${optional.slice(0, 4).map((k) => `"${k}"`).join(', ')}.` : ''}`;
}

// Small models favour whatever is listed first, so the everyday tools lead.
const PREFERRED_ORDER = ['Read', 'Glob', 'Grep', 'Bash', 'PowerShell', 'Write', 'Edit'];
export const orderTools = (tools) => [...tools].sort((a, b) => {
  const ia = PREFERRED_ORDER.indexOf(a.name), ib = PREFERRED_ORDER.indexOf(b.name);
  return (ia === -1 ? 99 : ia) - (ib === -1 ? 99 : ib);
});

export function toolProtocol(tools) {
  return [
    'To use a tool, reply with only one JSON object and nothing else:',
    '{"name": "<tool name>", "parameters": {<arguments>}}',
    'Tools:',
    ...orderTools(tools).map(describeTool)
  ].join('\n');
}

// Worked examples, written into the system prompt as past tasks. Small models copy demonstrations
// far more reliably than they follow described formats — but given as real conversation turns, the
// model treated the example as the current conversation, so they live in the system prompt, labelled.
export function examples(tools) {
  const names = new Set(tools.map((t) => t.name));
  const out = [];
  if (names.has('Read') && names.has('Edit')) {
    out.push([
      'Example task: "Change the port in config/server.json to 9090."',
      `You: ${toolCallText('Read', { file_path: 'config/server.json' })}`,
      'Result: 1\t{ "host": "0.0.0.0", "port": 8080 }',
      `You: ${toolCallText('Edit', { file_path: 'config/server.json', old_string: '"port": 8080', new_string: '"port": 9090' })}`,
      'Result: The file config/server.json has been updated.',
      'You: Done. config/server.json now uses port 9090.'
    ].join('\n'));
  }
  if (names.has('Grep')) {
    out.push([
      'Example task: "Which files use formatDate?"',
      `You: ${toolCallText('Grep', { pattern: 'formatDate' })}`,
      'Result: src/app.js\nsrc/utils/date.js',
      'You: formatDate is used in src/app.js and src/utils/date.js.'
    ].join('\n'));
  }
  out.push(['Example task: "What does HTTP stand for?" (no tool needed)', 'You: HyperText Transfer Protocol.'].join('\n'));
  return out.join('\n\n');
}

// The JSON a tool call is written as, both when asking for one and when replaying history.
export const toolCallText = (name, input) => JSON.stringify({ name, parameters: input ?? {} });

// ---------------------------------------------------------------- parsing

// Every balanced top-level {...} in the text, with its position. Strings and escapes respected.
function jsonObjects(text) {
  const out = [];
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== '{') continue;
    let depth = 0, inStr = false, esc = false;
    for (let j = i; j < text.length; j++) {
      const c = text[j];
      if (inStr) {
        if (esc) esc = false;
        else if (c === '\\') esc = true;
        else if (c === '"') inStr = false;
        continue;
      }
      if (c === '"') inStr = true;
      else if (c === '{') depth++;
      else if (c === '}' && --depth === 0) { out.push({ start: i, end: j + 1, src: text.slice(i, j + 1) }); i = j; break; }
    }
  }
  return out;
}

// Raw tabs/newlines inside JSON strings are invalid JSON but a common slip; escape them.
function escapeControlInStrings(src) {
  let out = '', inStr = false, esc = false;
  for (const c of src) {
    if (inStr) {
      if (esc) { esc = false; out += c; continue; }
      if (c === '\\') { esc = true; out += c; continue; }
      if (c === '"') { inStr = false; out += c; continue; }
      out += c === '\n' ? '\\n' : c === '\r' ? '\\r' : c === '\t' ? '\\t' : c;
    } else {
      if (c === '"') inStr = true;
      out += c;
    }
  }
  return out;
}

function parseLoose(src) {
  try { return JSON.parse(src); } catch {}
  if (/[\t\r\n]/.test(src)) {
    try { return JSON.parse(escapeControlInStrings(src)); } catch {}
    src = escapeControlInStrings(src);
  }
  // Common slips from small models: trailing commas, single quotes, Python literals.
  const fixed = src
    .replace(/,\s*([}\]])/g, '$1')
    .replace(/\bTrue\b/g, 'true').replace(/\bFalse\b/g, 'false').replace(/\bNone\b/g, 'null');
  try { return JSON.parse(fixed); } catch {}
  try { return JSON.parse(fixed.replace(/'/g, '"')); } catch {}
  return null;
}

export function resolveToolName(name, tools) {
  if (!name || typeof name !== 'string') return null;
  const names = tools.map((t) => t.name);
  const exact = names.find((n) => n === name) || names.find((n) => n.toLowerCase() === name.toLowerCase());
  if (exact) return exact;
  const alias = TOOL_ALIASES[name.toLowerCase().replace(/[\s-]/g, '_')];
  return alias && names.includes(alias) ? alias : null;
}

/**
 * Pull every tool call out of the model's reply, in order.
 * Accepts {"name","parameters"} (also "arguments"/"input"/"args"), {"tool","input"}, arguments
 * written beside the name, Llama's <function=Name>{…}</function>, and JSON inside code fences;
 * ignores the <|python_tag|> marker.
 * @returns {{ calls: {name:string, input:object}[], text: string, problem?: string }}
 *   text is the prose around the calls (the whole reply when there are none).
 */
export function parseToolCalls(reply, tools) {
  let text = String(reply ?? '').replace(/<\|python_tag\|>/g, '').replace(/<\|eom_id\|>|<\|eot_id\|>/g, '');
  if (!tools?.length) return { calls: [], text: text.trim() };

  const calls = [];
  const spans = [];
  for (const fn of text.matchAll(/<function=([\w.-]+)>\s*(\{[\s\S]*?\})\s*<\/function>/g)) {
    const name = resolveToolName(fn[1], tools);
    const input = parseLoose(fn[2]);
    if (name && input) { calls.push({ name, input, at: fn.index }); spans.push([fn.index, fn.index + fn[0].length]); }
  }

  let problem;
  const parsedAt = new Set(); // starts of objects that were valid JSON (a call or not)
  for (const obj of jsonObjects(text)) {
    if (spans.some(([a, b]) => obj.start >= a && obj.end <= b)) continue;
    const v = parseLoose(obj.src);
    if (!v || typeof v !== 'object' || Array.isArray(v)) continue;
    parsedAt.add(obj.start);
    const rawName = v.name ?? v.tool ?? v.tool_name ?? v.function?.name ?? v.function;
    let rawInput = v.parameters ?? v.arguments ?? v.input ?? v.args ?? v.params ?? v.function?.arguments;
    if (typeof rawName !== 'string') continue;
    const name = resolveToolName(rawName, tools);
    if (!name) {
      // Only an object shaped like a call counts as a bad call; {"name": "Ada"} may just be an answer.
      if (rawInput !== undefined) problem = `There is no tool named "${rawName}". Available tools: ${tools.map((t) => t.name).join(', ')}.`;
      continue;
    }
    // {"name": "Bash", "command": "ls"} — arguments written beside the name instead of under "parameters".
    if (rawInput === undefined) {
      const { name: _n, tool: _t, tool_name: _tn, function: _f, ...rest } = v;
      rawInput = rest;
    }
    let input = typeof rawInput === 'string' ? parseLoose(rawInput) ?? { value: rawInput } : rawInput ?? {};
    if (typeof input !== 'object' || Array.isArray(input)) input = {};
    calls.push({ name, input, at: obj.start });
    spans.push([obj.start, obj.end]);
  }

  // A call that almost parses — a missing closing quote or brace — is the most common slip. Try the
  // few closings that fix it; if none does, tell the model its call was not valid JSON.
  if (!calls.length) {
    const at = text.search(/\{\s*"(?:name|tool)"\s*:/);
    if (at !== -1 && !parsedAt.has(at)) {
      const src = text.slice(at).trim();
      // Most likely first: a string left open before the final braces, then braces never closed.
      const closings = [src.replace(/\}\}\s*$/, '"}}'), src.replace(/\}\s*$/, '"}'), src + '}', src + '}}', src + '"}}', src + '"}'];
      for (const candidate of closings) {
        const v = parseLoose(candidate);
        const name = v && resolveToolName(v.name ?? v.tool, tools);
        const input = v && (v.parameters ?? v.arguments ?? v.input);
        if (name && input && typeof input === 'object') {
          calls.push({ name, input, at });
          spans.push([at, text.length]);
          break;
        }
      }
      if (!calls.length) problem = 'Your tool call was not valid JSON. Reply with one complete JSON object: {"name": "<tool>", "parameters": {…}}, with every quote and brace closed.';
    }
  }

  calls.sort((a, b) => a.at - b.at);
  let prose = text;
  for (const [a, b] of [...spans].sort((x, y) => y[0] - x[0])) prose = prose.slice(0, a) + '\n' + prose.slice(b);
  prose = prose.replace(/```(?:json)?\s*```/g, '').replace(/```(?:json)?/g, '').split('\n')
    .filter((l) => !/^[\W_\d]{0,4}$/.test(l.trim()) || !l.trim()).join('\n').replace(/\n{3,}/g, '\n\n').trim();
  return { calls: calls.map(({ name, input }) => ({ name, input })), text: calls.length ? prose : text.trim(), problem: calls.length ? undefined : problem };
}

/** The first tool call in the reply (see parseToolCalls). */
export function parseToolCall(reply, tools) {
  const r = parseToolCalls(reply, tools);
  return { call: r.calls[0] || null, text: r.text, problem: r.problem };
}

// ---------------------------------------------------------------- repair + validation

/**
 * Map aliased argument names onto the schema's, drop unknown ones, absolutise paths, and check
 * required arguments. Returns the fixed input plus a problem string when it still cannot run.
 */
export function repairToolInput(name, input, tools, { cwd } = {}) {
  // A call whose arguments belong to another tool ({"name":"Bash","parameters":{"file_path":…}})
  // meant that tool.
  const inferred = inferToolFromArgs(name, input, tools);
  if (inferred) name = inferred;
  const tool = tools.find((t) => t.name === name);
  const props = tool?.input_schema?.properties || {};
  const required = tool?.input_schema?.required || [];
  const out = {};
  for (const [k, v] of Object.entries(input || {})) {
    if (k in props) { out[k] = v; continue; }
    const target = Object.keys(PARAM_ALIASES).find((p) => p in props && PARAM_ALIASES[p].includes(k) && !(p in input));
    if (target && !(target in out)) out[target] = v;
    else if (!Object.keys(props).length) out[k] = v; // schema-less tool: pass through
  }
  // A lone unnamed argument ({"value": …}) goes to the only required parameter.
  if ('value' in (input || {}) && required.length === 1 && !(required[0] in out)) out[required[0]] = input.value;

  // Coerce scalar types the schema asks for, and fit enum values ("list" → "files_with_matches").
  for (const [k, v] of Object.entries(out)) {
    const allowed = props[k]?.enum;
    if (Array.isArray(allowed) && !allowed.includes(v)) {
      const s = String(v).toLowerCase();
      const match = allowed.find((a) => String(a).toLowerCase() === s)
        ?? ENUM_ALIASES[s]?.find((a) => allowed.includes(a))
        ?? allowed.find((a) => String(a).toLowerCase().startsWith(s) || s.startsWith(String(a).toLowerCase()));
      if (match !== undefined) out[k] = match;
      else delete out[k];
      continue;
    }
    const type = props[k]?.type;
    if (type === 'number' || type === 'integer') { const n = Number(v); if (Number.isFinite(n)) out[k] = type === 'integer' ? Math.trunc(n) : n; }
    else if (type === 'boolean' && typeof v === 'string') out[k] = v.toLowerCase() === 'true';
    else if (type === 'string' && v !== null && typeof v === 'object') out[k] = JSON.stringify(v);
    else if (type === 'string' && typeof v !== 'string' && v != null) out[k] = String(v);
  }

  // Line numbers copied from a Read result ("1\texport function…") into text meant for the file.
  for (const k of ['content', 'old_string', 'new_string']) {
    if (typeof out[k] === 'string') out[k] = stripNumberColumn(out[k]);
  }
  // Grep's glob given a plain file ("src/math.js") meant "search this file": that is path.
  if (name === 'Grep' && typeof out.glob === 'string' && !/[*?[{]/.test(out.glob) && /[\\/.]/.test(out.glob) && !out.path) {
    out.path = out.glob;
    delete out.glob;
  }
  // Git Bash eats backslashes: C:\Users\x → C:/Users/x inside Bash commands.
  if (name === 'Bash' && typeof out.command === 'string') {
    out.command = out.command.replace(/\b([A-Za-z]):\\((?:[^\s"'`\\]+\\?)+)/g, (m, d, rest) => `${d}:/${rest.replace(/\\/g, '/')}`);
  }
  if (cwd) {
    for (const p of ABSOLUTE_PATH_PARAMS[name] || []) if (typeof out[p] === 'string' && out[p]) out[p] = absolutise(out[p], cwd);
    for (const p of OPTIONAL_PATH_PARAMS[name] || []) {
      if (typeof out[p] === 'string' && out[p] && !['.', './'].includes(out[p])) out[p] = absolutise(out[p], cwd);
      else if (out[p] !== undefined && ['.', './', ''].includes(out[p])) delete out[p];
      // A search rooted at "/" or above the project scans the whole drive (seen: a 20 s ripgrep
      // timeout on C:\). Search the project instead.
      if (typeof out[p] === 'string' && isAncestorOrSelf(out[p], cwd)) delete out[p];
    }
  }

  const missing = required.filter((k) => out[k] === undefined || out[k] === null || (out[k] === '' && k !== 'new_string'));
  return {
    name,
    input: out,
    problem: missing.length ? `The ${name} tool needs ${missing.map((k) => `"${k}"`).join(' and ')}. Reply with the complete JSON call.` : null
  };
}

// "1\tfoo\n2\tbar" / "1   foo\n2   bar" → "foo\nbar", only when every non-empty line is numbered
// in sequence — so real content that starts with numbers is left alone.
function stripNumberColumn(s) {
  const lines = s.split('\n');
  const nonEmpty = lines.filter((l) => l.trim());
  if (nonEmpty.length < 1) return s;
  const re = /^\s*(\d+)(?:\t|→| {2,})/;
  const nums = nonEmpty.map((l) => l.match(re)?.[1]);
  if (nums.some((n) => n === undefined)) return s;
  if (!nums.every((n, i) => i === 0 || Number(n) === Number(nums[i - 1]) + 1)) return s;
  return lines.map((l) => l.replace(re, '')).join('\n');
}

// Canonical argument name for a key the model used (its own name if it is not an alias).
const canonical = (k) => Object.keys(PARAM_ALIASES).find((p) => p === k || PARAM_ALIASES[p].includes(k)) || k;

function inferToolFromArgs(name, input, tools) {
  const keys = Object.keys(input || {});
  if (!keys.length) return null;
  const fits = (t) => {
    const props = Object.keys(t.input_schema?.properties || {});
    const required = t.input_schema?.required || [];
    const mapped = keys.map((k) => (props.includes(k) ? k : props.includes(canonical(k)) ? canonical(k) : null));
    return mapped.every(Boolean) && required.every((r) => mapped.includes(r));
  };
  const current = tools.find((t) => t.name === name);
  if (current && fits(current)) return null;
  // Any argument the named tool knows means the model did mean that tool, just incompletely.
  const currentProps = Object.keys(current?.input_schema?.properties || {});
  if (keys.some((k) => currentProps.includes(k) || currentProps.includes(canonical(k)))) return null;
  // Prefer the tool with the most required arguments satisfied (Edit over Read for file_path+old_string…).
  const candidates = tools.filter(fits).sort((a, b) => (b.input_schema?.required?.length || 0) - (a.input_schema?.required?.length || 0));
  return candidates[0]?.name || null;
}

function isAncestorOrSelf(p, cwd) {
  const n = (s) => {
    const t = s.replace(/\\/g, '/').replace(/\/+$/, '') || '/';
    return /^[a-zA-Z]:/.test(t) ? t.toLowerCase() : t;
  };
  const a = n(p), c = n(cwd);
  return a === c || c.startsWith(a.endsWith('/') ? a : a + '/') || /^[a-z]:$/.test(a);
}

function absolutise(p, cwd) {
  let v = p.trim().replace(/^["']|["']$/g, '');
  if (v === '~' || v.startsWith('~/')) return v; // leave home-relative paths to the tool
  const win = /^[a-zA-Z]:[\\/]/.test(cwd) || cwd.startsWith('\\\\');
  if (win) {
    if (/^[a-zA-Z]:[\\/]/.test(v) || v.startsWith('\\\\')) return v;
    // A Git Bash path (/c/Users/...) is absolute too.
    const m = v.match(/^\/([a-zA-Z])\/(.*)$/);
    if (m) return `${m[1].toUpperCase()}:\\${m[2].replace(/\//g, '\\')}`;
    return path.win32.resolve(cwd, v.replace(/^\.\//, ''));
  }
  return path.posix.isAbsolute(v) ? v : path.posix.resolve(cwd, v);
}
