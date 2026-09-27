#!/usr/bin/env node
// End-to-end eval for the ChatJimmy provider: each task runs the real launcher
// (`bro -p chatjimmy --claude --print …`) in a fresh fixture project, then checks the answer
// and/or the files Claude Code left behind.
//
//   node scripts/chatjimmy-eval.js                 # every task once
//   node scripts/chatjimmy-eval.js --only write-file,edit-json --repeat 3
//   node scripts/chatjimmy-eval.js --keep          # keep workspaces + traces for inspection
//
// Sessions are not persisted (--no-session-persistence), so runs leave no Claude Code history.

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const BRO = path.join(ROOT, 'bin', 'bro.js');

const { values: opts } = parseArgs({
  options: {
    only: { type: 'string' },
    repeat: { type: 'string', default: '1' },
    keep: { type: 'boolean', default: false },
    timeout: { type: 'string', default: '240' },
    model: { type: 'string', default: 'llama3.1-8B' },
    out: { type: 'string' }
  }
});

const bigFile = () => {
  const lines = ['// Generated helpers.'];
  for (let i = 1; i <= 60; i++) lines.push(`export function helper${i}(x) {`, `  // helper ${i} scales its input`, `  return x * ${i};`, '}', '');
  lines.push('export function getLimit() {', '  return 4096;', '}', '');
  return lines.join('\n');
};

const FIXTURE = {
  'package.json': JSON.stringify({ name: 'demo-app', version: '2.4.1', type: 'module', scripts: { test: 'node test.js' } }, null, 2) + '\n',
  'src/math.js': 'export function add(a, b) { return a + b; }\nexport function mul(a, b) { return a * b; }\n',
  'src/calc.js': "import { add } from './math.js';\nexport const total = (xs) => xs.reduce(add, 0);\n",
  'src/report.js': "import { mul } from './math.js';\nexport const area = (w, h) => mul(w, h);\n",
  'src/big.js': bigFile(),
  'test.js': "import { add, mul } from './src/math.js';\nif (add(2, 3) !== 5 || mul(2, 3) !== 6) { console.error('FAIL'); process.exit(1); }\nconsole.log('all tests passed');\n",
  'README.md': '# Demo app\n\nA tiny demo used to test coding agents.\n',
  'notes/todo.txt': '- write the docs\n- add a sub() helper\n- publish 2.5.0\n'
};

const read = (ws, f) => { try { return fs.readFileSync(path.join(ws, f), 'utf8'); } catch { return null; } };
const has = (s, re) => re.test(s || '');

// Each check returns true or a short reason it failed.
const TASKS = [
  { id: 'arithmetic', prompt: 'What is 17 * 3? Reply with just the number.', check: ({ out }) => has(out, /\b51\b/) || 'answer lacks 51' },
  { id: 'chat', prompt: 'Explain in one sentence what a JavaScript closure is.', check: ({ out }) => (out.length > 30 && has(out, /function|scope|variable/i)) || 'no sensible explanation' },
  { id: 'read-version', prompt: 'What version is this project? Check package.json.', check: ({ out }) => has(out, /2\.4\.1/) || 'answer lacks 2.4.1' },
  { id: 'exports', prompt: 'Which functions does src/math.js export?', check: ({ out }) => (has(out, /\badd\b/) && has(out, /\bmul\b/)) || 'answer lacks add and mul' },
  { id: 'list-src', prompt: 'What files are in the src folder?', check: ({ out }) => has(out, /math\.js/) || 'answer lacks math.js' },
  { id: 'grep', prompt: "Which file contains the phrase 'tiny demo'?", check: ({ out }) => has(out, /README/i) || 'answer lacks README' },
  { id: 'count-todo', prompt: 'How many items are in notes/todo.txt?', check: ({ out }) => has(out, /\b3\b|\bthree\b/i) || 'answer lacks 3' },
  { id: 'run-cmd', prompt: 'Run the command node -e "console.log(6*7)" and tell me what it printed.', check: ({ out }) => has(out, /\b42\b/) || 'answer lacks 42' },
  {
    id: 'write-file',
    prompt: 'Create a file named hello.txt containing exactly the text: hello world',
    check: ({ ws }) => (read(ws, 'hello.txt')?.trim() === 'hello world') || `hello.txt is ${JSON.stringify(read(ws, 'hello.txt'))}`
  },
  {
    id: 'edit-json',
    prompt: 'In package.json, change the version to 2.5.0.',
    check: ({ ws }) => {
      try { const p = JSON.parse(read(ws, 'package.json')); return (p.version === '2.5.0' && p.name === 'demo-app') || `version is ${p.version}`; }
      catch { return 'package.json is no longer valid JSON'; }
    }
  },
  {
    id: 'edit-add-fn',
    prompt: 'Add a function sub(a, b) that returns a - b to src/math.js, exported like the others.',
    check: ({ ws }) => {
      const s = read(ws, 'src/math.js') || '';
      if (!has(s, /export\s+(function\s+sub\s*\(|const\s+sub\s*=)/)) return 'no exported sub';
      if (!has(s, /export\s+function\s+add/) || !has(s, /export\s+function\s+mul/)) return 'add/mul were lost';
      return true;
    }
  },
  {
    id: 'fix-bug',
    setup: (ws) => fs.writeFileSync(path.join(ws, 'src/math.js'), 'export function add(a, b) { return a + b; }\nexport function mul(a, b) { return a + b; }\n'),
    prompt: 'The mul function in src/math.js returns the wrong result. Fix it.',
    check: ({ ws }) => {
      const s = read(ws, 'src/math.js') || '';
      return (has(s, /function\s+mul\s*\(\s*a\s*,\s*b\s*\)\s*\{\s*return\s+a\s*\*\s*b/) && has(s, /function\s+add\s*\(\s*a\s*,\s*b\s*\)\s*\{\s*return\s+a\s*\+\s*b/)) || `math.js is ${JSON.stringify(s)}`;
    }
  },
  {
    id: 'rename',
    prompt: 'Rename the function mul to multiply in src/math.js.',
    check: ({ ws }) => {
      const s = read(ws, 'src/math.js') || '';
      return (has(s, /function\s+multiply\s*\(/) && !has(s, /function\s+mul\s*\(/) && has(s, /function\s+add\s*\(/)) || `math.js is ${JSON.stringify(s)}`;
    }
  },
  {
    id: 'create-run',
    prompt: "Create a script hello.js that prints 'hi from jimmy', then run it with node and tell me the output.",
    check: ({ ws, out }) => (has(read(ws, 'hello.js'), /hi from jimmy/) && has(out, /hi from jimmy/i)) || `hello.js=${JSON.stringify(read(ws, 'hello.js'))}`
  },
  { id: 'run-tests', prompt: 'Run npm test and tell me whether the tests pass.', check: ({ out }) => (has(out, /pass/i) && !has(out, /\bfail(ed|s)?\b(?!.*pass)/i)) || 'answer does not say the tests pass' },
  { id: 'importers', prompt: 'Which files import from ./math.js?', check: ({ out }) => (has(out, /calc\.js/) && has(out, /report\.js/)) || 'answer lacks calc.js and report.js' },
  { id: 'long-file', prompt: 'What value does getLimit() in src/big.js return?', check: ({ out }) => has(out, /\b4096\b/) || 'answer lacks 4096' },
  {
    // Follow-up turns (claude --continue) must act on the earlier conversation — and really act:
    // a change claimed in the answer but never made is the failure this guards against.
    id: 'follow-up',
    turns: ['Read package.json and tell me its version.', 'Now bump the patch number of that version by one.', 'What was the version before you changed it?'],
    check: ({ ws, outs }) => {
      let v;
      try { v = JSON.parse(read(ws, 'package.json')).version; } catch { return 'package.json is no longer valid JSON'; }
      if (v !== '2.4.2') return `version is ${v} after the bump`;
      return (has(outs[0], /2\.4\.1/) && has(outs[2], /2\.4\.1/)) || `answers: ${JSON.stringify(outs)}`;
    }
  },
  {
    id: 'multi-step',
    prompt: 'Read src/math.js, then create src/math.test.js that imports add from ./math.js and checks that add(2, 3) === 5 using console.assert.',
    check: ({ ws }) => {
      const s = read(ws, 'src/math.test.js');
      if (!s) return 'src/math.test.js missing';
      return (has(s, /import\s*\{[^}]*\badd\b[^}]*\}\s*from\s*['"]\.\/math(\.js)?['"]/) && has(s, /add\(\s*2\s*,\s*3\s*\)/)) || 'test file lacks the import or the check';
    }
  }
];

// One headless run. Multi-turn tasks keep a session (in a throwaway CLAUDE_CONFIG_DIR) and continue it.
function runBro(ws, prompt, traceDir, { configDir = null, cont = false } = {}) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const session = configDir ? (cont ? ['--continue'] : []) : ['--no-session-persistence'];
    const child = spawn(process.execPath, [BRO, '-p', 'chatjimmy', '-m', opts.model, '--claude', '--print', prompt, ...session], {
      cwd: ws,
      env: { ...process.env, BRO_CHATJIMMY_TRACE: traceDir, ...(configDir && { CLAUDE_CONFIG_DIR: configDir }) },
      stdio: ['ignore', 'pipe', 'pipe']
    });
    let out = '', err = '';
    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    const timer = setTimeout(() => { child.kill(); err += '\n[eval] timed out'; }, Number(opts.timeout) * 1000);
    child.on('exit', (code) => { clearTimeout(timer); resolve({ code, out: out.trim(), err, ms: Date.now() - t0 }); });
  });
}

function summariseTrace(dir) {
  if (!fs.existsSync(dir)) return { requests: 0, calls: [], fixes: 0 };
  const replies = fs.readdirSync(dir).filter((f) => f.endsWith('-reply.json')).sort();
  const calls = [];
  let fixes = 0, steered = 0;
  for (const f of replies) {
    const d = JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8'));
    fixes += d.fixes?.length || 0;
    steered += (d.notes || []).filter((n) => /^jev: /.test(n)).length;
    for (const b of d.result?.content || []) if (b.type === 'tool_use') calls.push(b.name);
  }
  return { requests: replies.length, calls, fixes, steered };
}

const selected = opts.only ? TASKS.filter((t) => opts.only.split(',').includes(t.id)) : TASKS;
const repeat = Number(opts.repeat);
const base = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-chatjimmy-eval-'));
const rows = [];

for (let r = 1; r <= repeat; r++) {
  for (const task of selected) {
    const ws = path.join(base, `${task.id}-${r}`, 'project');
    const traceDir = path.join(base, `${task.id}-${r}`, 'trace');
    for (const [f, c] of Object.entries(FIXTURE)) {
      fs.mkdirSync(path.dirname(path.join(ws, f)), { recursive: true });
      fs.writeFileSync(path.join(ws, f), c);
    }
    task.setup?.(ws);
    let res, outs = [];
    if (task.turns) {
      const configDir = path.join(base, `${task.id}-${r}`, 'claude-config');
      fs.mkdirSync(configDir, { recursive: true });
      let ms = 0;
      for (const [i, turn] of task.turns.entries()) {
        res = await runBro(ws, turn, traceDir, { configDir, cont: i > 0 });
        ms += res.ms;
        outs.push(res.out);
        if (res.code !== 0) break;
      }
      res = { ...res, ms, out: outs.join(' ⏎ ') };
    } else {
      res = await runBro(ws, task.prompt, traceDir);
    }
    const verdict = res.code === 0 ? task.check({ out: res.out, outs, ws }) : `exit ${res.code}: ${res.err.trim().split('\n').pop()}`;
    const t = summariseTrace(traceDir);
    const pass = verdict === true;
    rows.push({ task: task.id, run: r, pass, ms: res.ms, requests: t.requests, tools: t.calls.join('>') || '-', fixes: t.fixes, why: pass ? '' : verdict, answer: res.out.replace(/\s+/g, ' ').slice(0, 90) });
    console.log(`${pass ? 'PASS' : 'FAIL'}  ${task.id.padEnd(12)} r${r}  ${String(Math.round(res.ms / 1000)).padStart(3)}s  req=${t.requests} tools=${t.calls.join('>') || '-'}${t.fixes ? ` fixes=${t.fixes}` : ''}${t.steered ? ` jev=${t.steered}` : ''}${pass ? '' : `  ← ${verdict}`}`);
    console.log(`      ${JSON.stringify(res.out.replace(/\s+/g, ' ').slice(0, 140))}`);
  }
}

const passed = rows.filter((r) => r.pass).length;
console.log(`\n${passed}/${rows.length} passed (${Math.round((100 * passed) / rows.length)}%) · median ${Math.round(rows.map((r) => r.ms).sort((a, b) => a - b)[Math.floor(rows.length / 2)] / 1000)}s per task`);
if (opts.out) fs.writeFileSync(opts.out, JSON.stringify({ at: new Date().toISOString(), rows }, null, 2));
if (opts.keep) console.log(`Workspaces and traces kept in ${base}`);
else fs.rmSync(base, { recursive: true, force: true });
