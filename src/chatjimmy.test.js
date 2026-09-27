import { expect, test } from 'bun:test';
import { parseToolCall, repairToolInput, describeTool, orderTools } from './chatjimmy/tools.js';
import { buildPrompt, fitToBudget, relativisePaths, estimateTokens, CLAUDE_SYSTEM_PROMPT } from './chatjimmy/prompt.js';
import { createAnthropicHandler } from './chatjimmy/anthropic.js';
import { jimmyChat, ContextOverflowError } from './chatjimmy/client.js';
import { serveFetch } from './chatjimmy/server.js';
import { leanClaudeArgs, chatJimmyEnv, runChatJimmy, CHATJIMMY_PROVIDER } from './chatjimmy.js';

const schema = (props, required = props) => ({ properties: Object.fromEntries(props.map((p) => [p, { type: 'string' }])), required });
const TOOLS = [
  { name: 'Bash', description: 'Run a command.', input_schema: schema(['command', 'description'], ['command']) },
  { name: 'Edit', input_schema: schema(['file_path', 'old_string', 'new_string']) },
  { name: 'Glob', input_schema: schema(['pattern', 'path'], ['pattern']) },
  { name: 'Grep', input_schema: schema(['pattern', 'path', 'glob'], ['pattern']) },
  { name: 'Read', input_schema: { properties: { file_path: { type: 'string' }, offset: { type: 'integer' } }, required: ['file_path'] } },
  { name: 'Write', input_schema: schema(['file_path', 'content']) }
];
const WIN = 'C:\\Users\\me\\proj';

// ---------------------------------------------------------------- tool-call parsing

test('parses Llama-style calls, top-level arguments, fences and prose', () => {
  expect(parseToolCall('{"name": "Read", "parameters": {"file_path": "a.js"}}', TOOLS).call).toEqual({ name: 'Read', input: { file_path: 'a.js' } });
  expect(parseToolCall('{"name": "Bash", "command": "ls"}', TOOLS).call).toEqual({ name: 'Bash', input: { command: 'ls' } });
  expect(parseToolCall('<function=Glob>{"pattern": "*.md"}</function>', TOOLS).call).toEqual({ name: 'Glob', input: { pattern: '*.md' } });
  const fenced = parseToolCall('Let me look.\n```json\n{"name":"read_file","arguments":{"path":"x"}}\n```', TOOLS);
  expect(fenced.call.name).toBe('Read');
  expect(fenced.text).toBe('Let me look.');
  expect(parseToolCall("{'name': 'Bash', 'parameters': {'command': 'pwd',}}", TOOLS).call.input.command).toBe('pwd');
});

test('plain answers and JSON answers are not tool calls', () => {
  expect(parseToolCall('The version is 2.4.1.', TOOLS)).toEqual({ call: null, text: 'The version is 2.4.1.', problem: undefined });
  const r = parseToolCall('{"name": "Ada", "age": 3}', TOOLS);
  expect(r.call).toBeNull();
  expect(r.problem).toBeUndefined();
  expect(parseToolCall('{"name": "Deploy", "parameters": {}}', TOOLS).problem).toMatch(/no tool named "Deploy"/);
});

test('repairs aliased arguments, infers the intended tool, absolutises paths', () => {
  expect(repairToolInput('Read', { path: 'src/a.js' }, TOOLS, { cwd: WIN })).toEqual({ name: 'Read', input: { file_path: `${WIN}\\src\\a.js` }, problem: null });
  // Bash called with Read's argument meant Read.
  expect(repairToolInput('Bash', { file_path: 'package.json' }, TOOLS, { cwd: WIN }).name).toBe('Read');
  expect(repairToolInput('Glob', { pattern: '*.js', path: '.' }, TOOLS, { cwd: WIN }).input).toEqual({ pattern: '*.js' });
  expect(repairToolInput('Read', { file_path: '/c/Users/me/x.txt' }, TOOLS, { cwd: WIN }).input.file_path).toBe('C:\\Users\\me\\x.txt');
  expect(repairToolInput('Read', { file_path: 'a.js' }, TOOLS, { cwd: '/home/me/p' }).input.file_path).toBe('/home/me/p/a.js');
  expect(repairToolInput('Read', { offset: '3', file_path: 'a' }, TOOLS, {}).input.offset).toBe(3);
  expect(repairToolInput('Edit', { file_path: 'a' }, TOOLS, {}).problem).toMatch(/needs "old_string" and "new_string"/);
  // Git Bash eats backslashes in Windows paths.
  expect(repairToolInput('Bash', { command: 'cat C:\\Users\\me\\proj\\a.txt' }, TOOLS, { cwd: WIN }).input.command).toBe('cat C:/Users/me/proj/a.txt');
});

test('known tools get one-line descriptions, unknown ones one from their schema; Read leads', () => {
  expect(describeTool(TOOLS[4])).toMatch(/^- Read \{"file_path"/);
  expect(describeTool({ name: 'Deploy', description: 'Ship it to prod. Long text…', input_schema: schema(['env']) })).toBe('- Deploy {"env": <string>} — Ship it to prod.');
  expect(orderTools(TOOLS).map((t) => t.name)).toEqual(['Read', 'Glob', 'Grep', 'Bash', 'Write', 'Edit']);
});

// ---------------------------------------------------------------- prompt building

const claudeBody = (extra = []) => ({
  model: 'llama3.1-8B',
  system: [{ type: 'text', text: 'x-anthropic-billing-header: cc_version=2' }, { type: 'text', text: CLAUDE_SYSTEM_PROMPT }],
  tools: TOOLS,
  messages: [
    { role: 'user', content: [{ type: 'text', text: '<system-reminder>\nAttribution rules…\n</system-reminder>' }, { type: 'text', text: 'List the src folder' }] },
    { role: 'system', content: `# Environment\n - Primary working directory: ${WIN}\n - Platform: win32\nToday's date is 2026-09-18.` },
    ...extra
  ]
});

test('agent prompt: environment extracted, boilerplate and reminders dropped, tools described', () => {
  const p = buildPrompt(claudeBody());
  expect(p.env.cwd).toBe(WIN);
  expect(p.system).toContain(`working in ${WIN} on win32`);
  expect(p.system).toContain('Today is 2026-09-18');
  expect(p.system).toContain('- Read {"file_path"');
  expect(p.system).not.toContain('billing');
  expect(p.system).not.toContain('bro ChatJimmy mode');
  expect(p.messages).toEqual([{ role: 'user', content: 'List the src folder' }]);
});

test('replayed calls and results show paths relative to the working directory', () => {
  const p = buildPrompt(claudeBody([
    { role: 'assistant', content: [{ type: 'tool_use', id: 't1', name: 'Glob', input: { pattern: '*.js', path: `${WIN}\\src` } }] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 't1', content: `${WIN}\\src\\math.js\n${WIN}\\src\\util\\x.js` }] }
  ]));
  expect(p.messages[1]).toEqual({ role: 'assistant', content: '{"name":"Glob","parameters":{"pattern":"*.js","path":"src"}}' });
  expect(p.messages[2].content).toBe('Result of Glob src:\nsrc/math.js\nsrc/util/x.js');
  expect(relativisePaths(`see ${WIN.replace(/\\/g, '/')}/a.txt`, WIN)).toBe('see a.txt');
});

test('fitting trims old tool output first and drops middle steps, keeping task and latest turn', () => {
  const big = 'line of output\n'.repeat(3000);
  const turns = [{ role: 'user', content: 'the task', kind: 'task' }];
  for (let i = 0; i < 6; i++) {
    turns.push({ role: 'assistant', content: `{"name":"Bash","parameters":{"command":"step ${i}"}}`, kind: 'text' });
    turns.push({ role: 'user', content: `Result of Bash step ${i}:\n${big}`, kind: 'result' });
  }
  const f = fitToBudget('system prompt', turns, 1500);
  expect(f.tokens).toBeLessThanOrEqual(1500);
  expect(f.messages[0].content).toStartWith('the task');
  expect(f.messages.at(-1).content).toContain('step 5');
  expect(f.messages.at(-1).content).toContain('omitted');
});

test('requests without tools keep the caller\'s own system prompt', () => {
  const p = buildPrompt({ system: 'Summarise the conversation in 5 words.', messages: [{ role: 'user', content: 'hello there' }] });
  expect(p.system).toBe('Summarise the conversation in 5 words.');
  expect(p.tools).toEqual([]);
});

// ---------------------------------------------------------------- Anthropic endpoint

const fakeChat = (replies) => {
  const calls = [];
  const fn = async (args) => {
    calls.push(args);
    const r = replies[Math.min(calls.length - 1, replies.length - 1)];
    if (r instanceof Error) throw r;
    return { text: r, stats: { prefill_tokens: 100, decode_tokens: 10 }, latencyMs: 1 };
  };
  fn.calls = calls;
  return fn;
};
const post = (handler, body, headers = {}) =>
  handler(new Request('http://x/v1/messages?beta=true', { method: 'POST', headers: { 'content-type': 'application/json', ...headers }, body: JSON.stringify(body) }));

test('a tool call becomes a tool_use block with an absolute path (non-streaming)', async () => {
  const handler = createAnthropicHandler({ chat: fakeChat(['{"name": "Read", "parameters": {"file_path": "package.json"}}']) });
  const res = await (await post(handler, claudeBody())).json();
  expect(res.stop_reason).toBe('tool_use');
  expect(res.content[0]).toMatchObject({ type: 'tool_use', name: 'Read', input: { file_path: `${WIN}\\package.json` } });
  expect(res.content[0].id).toStartWith('toolu_');
});

test('streams Anthropic SSE with text then tool_use', async () => {
  const handler = createAnthropicHandler({ chat: fakeChat(['Checking.\n{"name":"Bash","parameters":{"command":"npm test"}}']) });
  const text = await (await post(handler, { ...claudeBody(), stream: true })).text();
  const events = text.split('\n\n').filter(Boolean).map((e) => JSON.parse(e.split('\ndata: ')[1]));
  expect(events.map((e) => e.type)).toEqual(['message_start', 'content_block_start', 'content_block_delta', 'content_block_stop', 'content_block_start', 'content_block_delta', 'content_block_stop', 'message_delta', 'message_stop']);
  expect(events[2].delta.text).toBe('Checking.');
  expect(JSON.parse(events[5].delta.partial_json)).toEqual({ command: 'npm test' });
  expect(events[7].delta.stop_reason).toBe('tool_use');
});

test('a broken call is sent back to the model to fix before Claude Code sees it', async () => {
  const chat = fakeChat(['{"name": "Edit", "parameters": {"file_path": "a.js"}}', '{"name": "Edit", "parameters": {"file_path": "a.js", "old_string": "x", "new_string": "y"}}']);
  const res = await (await post(createAnthropicHandler({ chat }), claudeBody())).json();
  expect(res.content[0].input).toMatchObject({ old_string: 'x', new_string: 'y' });
  expect(chat.calls[1].messages.at(-1).content).toMatch(/needs "old_string" and "new_string"/);
});

test('an empty reply (context overflow) is retried with a smaller prompt', async () => {
  const chat = fakeChat([new ContextOverflowError(), 'Done.']);
  const res = await (await post(createAnthropicHandler({ chat, budget: 4000 }), claudeBody())).json();
  expect(res.content).toEqual([{ type: 'text', text: 'Done.' }]);
  expect(chat.calls).toHaveLength(2);
});

test('the endpoint checks its per-launch token and answers liveness and count_tokens', async () => {
  const handler = createAnthropicHandler({ chat: fakeChat(['hi']), token: 'secret' });
  expect((await post(handler, claudeBody())).status).toBe(401);
  expect((await post(handler, claudeBody(), { 'x-api-key': 'secret' })).status).toBe(200);
  expect((await handler(new Request('http://x/api/hello', { method: 'HEAD' }))).status).toBe(200);
  const count = await handler(new Request('http://x/v1/messages/count_tokens', { method: 'POST', headers: { authorization: 'Bearer secret' }, body: JSON.stringify(claudeBody()) }));
  expect((await count.json()).input_tokens).toBeGreaterThan(50);
});

test('client strips the stats sentinel and reports an empty reply as context overflow', async () => {
  const stream = (parts) => async () => new Response(new ReadableStream({ start(c) { for (const p of parts) c.enqueue(new TextEncoder().encode(p)); c.close(); } }));
  const deltas = [];
  const r = await jimmyChat({ messages: [], onText: (d) => deltas.push(d), fetchImpl: stream(['Hel', 'lo<|st', 'ats|>{"decode_tokens":2}<|/stats|>']) });
  expect(r.text).toBe('Hello');
  expect(deltas.join('')).toBe('Hello');
  expect(r.stats).toEqual({ decode_tokens: 2 });
  await expect(jimmyChat({ messages: [], fetchImpl: stream([]) })).rejects.toBeInstanceOf(ContextOverflowError);
});

test('serveFetch serves the handler over HTTP on an ephemeral loopback port', async () => {
  const server = await serveFetch(createAnthropicHandler({ chat: fakeChat(['pong']) }));
  try {
    expect(server.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
    const res = await fetch(`${server.url}/v1/messages`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(claudeBody()) });
    expect((await res.json()).content[0].text).toBe('pong');
  } finally {
    await server.close();
  }
});

// ---------------------------------------------------------------- bro integration

test('Claude Code is launched lean against the local endpoint', () => {
  const args = leanClaudeArgs();
  for (const flag of ['--strict-mcp-config', '--disable-slash-commands', '--no-chrome', '--setting-sources', '--tools', '--system-prompt']) expect(args).toContain(flag);
  expect(args[args.indexOf('--setting-sources') + 1]).toBe('');
  const env = chatJimmyEnv('http://127.0.0.1:1234', 'tok', 'llama3.1-8B', { ANTHROPIC_API_KEY: 'sk-real', PATH: 'x' });
  expect(env.ANTHROPIC_API_KEY).toBeUndefined();
  expect(env).toMatchObject({ ANTHROPIC_BASE_URL: 'http://127.0.0.1:1234', ANTHROPIC_AUTH_TOKEN: 'tok', ANTHROPIC_DEFAULT_HAIKU_MODEL: 'llama3.1-8B' });
  expect(CHATJIMMY_PROVIDER).toMatchObject({ id: 'chatjimmy', mode: 'chatjimmy', noKey: true });
});

test('dry run describes the lean launch; codex is refused', async () => {
  const out = await runChatJimmy({ dryRun: true, permissionMode: 'bypass', extraArgs: ['-p', 'hi'] });
  expect(out.args.slice(0, 3)).toEqual(['--dangerously-skip-permissions', '--model', 'llama3.1-8B']);
  expect(out.args.slice(-2)).toEqual(['-p', 'hi']);
  expect(await runChatJimmy({ harness: 'codex', dryRun: true })).toBe(1);
});

test('token estimate is conservative for prose', () => {
  // 400 lines of this filler measured 5,630 prefill tokens on ChatJimmy.
  const filler = Array.from({ length: 400 }, (_, i) => `Line ${i}: the quick brown fox jumps over the lazy dog.`).join('\n');
  expect(estimateTokens(filler)).toBeGreaterThan(5630);
});

// ---------------------------------------------------------------- policy (guard rails)

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { applyPolicy } from './chatjimmy/policy.js';

test('policy: an unread existing file is read before Edit/Write; new files are written directly', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cj-policy-'));
  const file = path.join(dir, 'package.json');
  fs.writeFileSync(file, '{\n  "name": "x",\n  "version": "1.0.0"\n}\n');
  const edit = { name: 'Edit', input: { file_path: file, old_string: '1.0.0', new_string: '2.0.0' } };
  expect(applyPolicy(edit, { messages: [], cwd: dir, tools: TOOLS }).call).toEqual({ name: 'Read', input: { file_path: file } });
  const readBefore = [{ role: 'assistant', content: [{ type: 'tool_use', id: 't', name: 'Read', input: { file_path: file.split(path.sep).join('/') } }] }];
  expect(applyPolicy(edit, { messages: readBefore, cwd: dir, tools: TOOLS }).call).toEqual(edit);
  const fresh = { name: 'Write', input: { file_path: path.join(dir, 'new.txt'), content: 'hi' } };
  expect(applyPolicy(fresh, { messages: [], cwd: dir, tools: TOOLS }).call).toEqual(fresh);
  // A Write that throws away most of a file it has read goes back to the model.
  const clobber = { name: 'Write', input: { file_path: file, content: '{"version": "2.0.0"}' } };
  expect(applyPolicy(clobber, { messages: readBefore, cwd: dir, tools: TOOLS }).problem).toMatch(/Use Edit/);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('policy: a shell command that only repeats the answer is dropped', () => {
  const answer = 'A JavaScript closure is a function that keeps access to the variables of the scope it was created in, even after that scope has returned.';
  const echo = { name: 'Bash', input: { command: `jake '${answer}'` } };
  expect(applyPolicy(echo, { messages: [], tools: TOOLS, text: answer }).call).toBeNull();
  expect(applyPolicy({ name: 'Bash', input: { command: 'npm test' } }, { messages: [], tools: TOOLS, text: answer }).call.name).toBe('Bash');
});

test('several independent calls in one reply are all kept; dependent ones are cut', async () => {
  const reply = '{"name": "Read", "parameters": {"file_path": "src/math.js"}}_0\n{"name": "Write", "parameters": {"file_path": "src/does-not-exist.test.js", "content": "x"}}\n{"name": "Bash", "parameters": {"command": "npm test"}}';
  const res = await (await post(createAnthropicHandler({ chat: fakeChat([reply]) }), claudeBody())).json();
  expect(res.content.map((b) => b.type === 'tool_use' ? b.name : b.text)).toEqual(['Read', 'Write']);
});

test('an answer smuggled into an invented command becomes the answer', async () => {
  const reply = `{"name": "Bash", "parameters": {"command": "jake -c 'A closure is a function that keeps access to the variables of the scope it was created in.'"}}`;
  const res = await (await post(createAnthropicHandler({ chat: fakeChat([reply]) }), claudeBody())).json();
  expect(res.stop_reason).toBe('end_turn');
  expect(res.content[0].text).toStartWith('A closure is a function');
});

test('long Read results show the parts the request names; copied line numbers and raw tabs are repaired', () => {
  const big = Array.from({ length: 120 }, (_, i) => `     ${i + 1}\tconst v${i} = ${i};`).join('\n') + '\n   121\texport function getLimit() {\n   122\t  return 4096;\n   123\t}';
  const p = buildPrompt({
    ...claudeBody(),
    messages: [
      { role: 'user', content: 'What value does getLimit() in big.js return?' },
      { role: 'system', content: `# Environment\n - Primary working directory: ${WIN}\n` },
      { role: 'assistant', content: [{ type: 'tool_use', id: 'b', name: 'Read', input: { file_path: `${WIN}\\big.js` } }] },
      { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'b', content: big }] }
    ]
  });
  const shown = p.messages.at(-1).content;
  expect(shown).toContain('mention getLimit');
  expect(shown).toContain('return 4096;');
  expect(shown).not.toContain('const v10 =');
  expect(shown).not.toMatch(/\t/);
  // Line numbers copied into Write content are removed; content that merely starts with numbers is kept.
  expect(repairToolInput('Write', { file_path: 'a.js', content: '1\tconst a = 1;\n2\tconst b = 2;' }, TOOLS, {}).input.content).toBe('const a = 1;\nconst b = 2;');
  expect(repairToolInput('Write', { file_path: 'a.txt', content: '3 apples\n7 pears' }, TOOLS, {}).input.content).toBe('3 apples\n7 pears');
  // A raw tab inside a JSON string no longer sinks the call.
  expect(parseToolCall('{"name": "Write", "parameters": {"file_path": "a.js", "content": "x\ty"}}', TOOLS).call.input.content).toBe('x\ty');
});

test('an answer after a file search must name what the search found', async () => {
  const body = claudeBody([
    { role: 'assistant', content: [{ type: 'tool_use', id: 'g', name: 'Grep', input: { pattern: 'math' } }] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'g', content: 'Found 2 files\nsrc/calc.js\nsrc/report.js' }] }
  ]);
  const chat = fakeChat(['src/math.js imports from math.js.', 'src/calc.js and src/report.js import from ./math.js.']);
  const res = await (await post(createAnthropicHandler({ chat }), body)).json();
  expect(res.content[0].text).toBe('src/calc.js and src/report.js import from ./math.js.');
  expect(chat.calls[1].messages.at(-1).content).toMatch(/does not name any of the files .*calc\.js, report\.js/);
});

test('policy: an "add" done by overwriting existing code becomes an append in the file\'s style', () => {
  const m = mathFixture();
  const overwrite = { name: 'Edit', input: { file_path: m.file, old_string: 'function add(a, b)', new_string: 'function sub(a, b) { return a - b; }' } };
  const r = applyPolicy(overwrite, m.ctx('Add a function sub(a, b) that returns a - b, exported like the others.'));
  expect(r.call.input.old_string).toBe('export function mul(a, b) { return a * b; }');
  expect(r.call.input.new_string).toBe('export function mul(a, b) { return a * b; }\nexport function sub(a, b) { return a - b; }');
  m.done();
});

test('policy: require() in an ES-module project is sent back; a cut-short string gets the quoting hint', () => {
  const m = mathFixture();
  fs.writeFileSync(path.join(m.dir, 'package.json'), '{ "type": "module" }');
  const req = { name: 'Write', input: { file_path: path.join(m.dir, 'math.test.js'), content: "const add = require('./math.js');\nconsole.assert(add(2, 3) === 5);" } };
  expect(applyPolicy(req, m.ctx('Create math.test.js')).problem).toMatch(/uses ES modules.*import \{ add \} from '\.\/math\.js'/);
  const cut = { name: 'Write', input: { file_path: path.join(m.dir, 'hello.js'), content: 'console.log(' } };
  expect(applyPolicy(cut, m.ctx('Create hello.js')).problem).toMatch(/double quote must be written as/);
  m.done();
});

test('a change claimed in the answer without any tool call is sent back', async () => {
  const body = claudeBody([
    { role: 'assistant', content: [{ type: 'tool_use', id: 'r', name: 'Read', input: { file_path: `${WIN}\\package.json` } }] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'r', content: '{ "version": "2.4.1" }' }] },
    { role: 'assistant', content: [{ type: 'text', text: 'The version is 2.4.1.' }] },
    { role: 'user', content: 'Now bump the patch number of that version by one.' }
  ]);
  const chat = fakeChat(['The version of package.json has been updated to 2.4.2.', '{"name": "Edit", "parameters": {"file_path": "package.json", "old_string": "2.4.1", "new_string": "2.4.2"}}']);
  const res = await (await post(createAnthropicHandler({ chat }), body)).json();
  expect(chat.calls[1].messages.at(-1).content).toMatch(/Nothing has been changed yet/);
  expect(res.content.at(-1)).toMatchObject({ type: 'tool_use', name: 'Edit' });
  // A question answered in the past tense is not a claim to check.
  const asked = claudeBody([{ role: 'user', content: 'What was the version before it was changed?' }]);
  const ok = fakeChat(['It was 2.4.1 before it was updated.']);
  expect((await (await post(createAnthropicHandler({ chat: ok }), asked)).json()).content[0].text).toMatch(/2\.4\.1/);
  expect(ok.calls).toHaveLength(1);
});

test('policy: search paths that do not exist, or narrow a first search without being asked, are dropped', () => {
  const m = mathFixture();
  fs.mkdirSync(path.join(m.dir, 'src'));
  expect(applyPolicy({ name: 'Grep', input: { pattern: 'import', path: path.join(m.dir, 'math-import-target.js') } }, m.ctx('Which files import math?')).call.input).toEqual({ pattern: 'import' });
  expect(applyPolicy({ name: 'Grep', input: { pattern: 'tiny demo', path: path.join(m.dir, 'src') } }, m.ctx("Which file contains 'tiny demo'?")).call.input).toEqual({ pattern: 'tiny demo' });
  expect(applyPolicy({ name: 'Grep', input: { pattern: 'x', path: path.join(m.dir, 'src') } }, m.ctx('Search src for x')).call.input.path).toBe(path.join(m.dir, 'src'));
  m.done();
});

test('Read results drop the numbered empty line after a final newline', () => {
  const p = buildPrompt(claudeBody([
    { role: 'assistant', content: [{ type: 'tool_use', id: 'r1', name: 'Read', input: { file_path: `${WIN}\\notes.txt` } }] },
    { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'r1', content: '     1\t- a\n     2\t- b\n     3\t- c\n     4\t' }] }
  ]));
  expect(p.messages.at(-1).content).toBe('Result of Read notes.txt:\n- a\n- b\n- c');
});

test('loop breakers: an unchanged repeat is refused, and the tools are withdrawn after the step limit', async () => {
  const read = { type: 'tool_use', id: 'a', name: 'Glob', input: { pattern: '**/*.txt' } };
  const result = { type: 'tool_result', tool_use_id: 'a', content: 'No files found' };
  const once = claudeBody([{ role: 'assistant', content: [read] }, { role: 'user', content: [result] }]);
  const chat = fakeChat(['{"name": "Glob", "parameters": {"pattern": "**/*.txt"}}', 'There are no .txt files.']);
  const res = await (await post(createAnthropicHandler({ chat }), once)).json();
  expect(chat.calls[1].messages.at(-1).content).toMatch(/already made exactly this Glob call/);
  expect(res.content).toEqual([{ type: 'text', text: 'There are no .txt files.' }]);

  const many = [];
  for (let i = 0; i < 20; i++) many.push({ role: 'assistant', content: [{ ...read, id: `s${i}`, input: { pattern: `*.${i}` } }] }, { role: 'user', content: [{ ...result, tool_use_id: `s${i}` }] });
  const p = buildPrompt(claudeBody(many));
  expect(p.tools).toEqual([]);
  expect(p.system).toMatch(/Tools are no longer available/);
});

test('searches rooted at the drive or above the project search the project instead', () => {
  expect(repairToolInput('Glob', { pattern: '*.md', path: '/' }, TOOLS, { cwd: WIN }).input).toEqual({ pattern: '*.md' });
  expect(repairToolInput('Grep', { pattern: 'x', path: 'C:\\Users' }, TOOLS, { cwd: WIN }).input).toEqual({ pattern: 'x' });
  expect(repairToolInput('Grep', { pattern: 'x', path: 'src' }, TOOLS, { cwd: WIN }).input.path).toBe(`${WIN}\\src`);
});

test('policy: an invented Grep file filter is dropped; one the request names is kept', () => {
  const ask = (text) => [{ role: 'user', content: text }];
  const grep = { name: 'Grep', input: { pattern: 'tiny demo', glob: '*.txt' } };
  expect(applyPolicy(grep, { messages: ask("Which file contains 'tiny demo'?"), tools: TOOLS }).call.input).toEqual({ pattern: 'tiny demo' });
  expect(applyPolicy(grep, { messages: ask('Which .txt file mentions it?'), tools: TOOLS }).call.input.glob).toBe('*.txt');
});

// A throwaway math.js the conversation has already read, for the file-level checks.
function mathFixture(source = 'export function add(a, b) { return a + b; }\nexport function mul(a, b) { return a * b; }\n') {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cj-math-'));
  const file = path.join(dir, 'math.js');
  fs.writeFileSync(file, source);
  const ctx = (request) => ({
    messages: [{ role: 'user', content: request }, { role: 'assistant', content: [{ type: 'tool_use', id: 'r', name: 'Read', input: { file_path: file } }] }],
    cwd: dir,
    tools: TOOLS
  });
  return { dir, file, ctx, done: () => fs.rmSync(dir, { recursive: true, force: true }) };
}

test('policy: an Edit that deletes a declaration the request did not ask to remove is sent back', () => {
  const m = mathFixture();
  const edit = { name: 'Edit', input: { file_path: m.file, old_string: 'export function add(a, b) { return a + b; }', new_string: 'export function sub(a, b) { return a - b; }' } };
  expect(applyPolicy(edit, m.ctx('Make math.js have a sub(a, b) function')).problem).toMatch(/deletes add\(\)/);
  // For an "add" request the same overwrite becomes an append (see the append-intent test).
  expect(applyPolicy(edit, m.ctx('Add a function sub(a, b)')).call.input.new_string).toContain('export function mul');
  expect(applyPolicy(edit, m.ctx('Replace add with sub')).problem).toBeUndefined();
  const append = { ...edit, input: { ...edit.input, new_string: edit.input.old_string + '\nexport function sub(a, b) { return a - b; }' } };
  expect(applyPolicy(append, m.ctx('Add a function sub(a, b)')).problem).toBeUndefined();
  // Off-target: the request is about sub(), the edit changes add()'s body.
  const offTarget = { name: 'Edit', input: { file_path: m.file, old_string: 'return a + b;', new_string: 'return a - b;' } };
  expect(applyPolicy(offTarget, m.ctx('Add a function sub(a, b) to math.js')).problem).toMatch(/about sub\(\)/);
  m.done();
});

test('policy: an ambiguous Edit is narrowed to the function the request names; replace_all may not touch others', () => {
  const m = mathFixture('export function add(a, b) { return a + b; }\nexport function mul(a, b) { return a + b; }\n');
  const fix = { name: 'Edit', input: { file_path: m.file, old_string: 'return a + b;', new_string: 'return a * b;' } };
  expect(applyPolicy(fix, m.ctx('The mul function returns the wrong result. Fix it.')).call.input).toEqual({
    file_path: m.file, old_string: 'export function mul(a, b) { return a + b; }', new_string: 'export function mul(a, b) { return a * b; }'
  });
  expect(applyPolicy(fix, m.ctx('Something returns the wrong result. Fix it.')).problem).toMatch(/appears 2 times .*lines 1, 2/);
  const all = { ...fix, input: { ...fix.input, replace_all: true } };
  expect(applyPolicy(all, m.ctx('The mul function returns the wrong result. Fix it.')).problem).toMatch(/also edits add\(\)/);
  m.done();
});

test('policy: a change that leaves the file unparseable is sent back', () => {
  const m = mathFixture();
  const broken = { name: 'Edit', input: { file_path: m.file, old_string: 'export function add(a, b) { return a + b; }', new_string: 'export function add(a, b) { return a + b; }\nsub(a, b) { return a - b; }' } };
  expect(applyPolicy(broken, m.ctx('Add a function sub(a, b)')).problem).toMatch(/would not parse/);
  const json = path.join(m.dir, 'package.json');
  fs.writeFileSync(json, '{\n  "version": "1.0.0"\n}\n');
  const readJson = { ...m.ctx('Set the version to 2.0.0'), messages: [{ role: 'user', content: 'Set the version to 2.0.0' }, { role: 'assistant', content: [{ type: 'tool_use', id: 'j', name: 'Read', input: { file_path: json } }] }] };
  expect(applyPolicy({ name: 'Edit', input: { file_path: json, old_string: '"1.0.0"', new_string: '2.0.0' } }, readJson).problem).toMatch(/would not parse/);
  expect(applyPolicy({ name: 'Edit', input: { file_path: json, old_string: '"1.0.0"', new_string: '"2.0.0"' } }, readJson).problem).toBeUndefined();
  m.done();
});

test('policy: questions do not change files; an "add" Write of only the new code becomes an append', () => {
  const ask = (text) => [{ role: 'user', content: text }];
  const edit = { name: 'Edit', input: { file_path: 'nope.js', old_string: 'a', new_string: 'b' } };
  expect(applyPolicy(edit, { messages: ask('Which functions does src/math.js export?'), tools: TOOLS }).problem).toMatch(/only asked a question/);
  expect(applyPolicy(edit, { messages: ask('Can you fix the typo in a.js?'), tools: TOOLS }).problem).toBeUndefined();

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'cj-append-'));
  const file = path.join(dir, 'math.js');
  fs.writeFileSync(file, 'export function add(a, b) { return a + b; }\nexport function mul(a, b) { return a * b; }\n');
  const read = [{ role: 'user', content: 'Add a function sub(a, b) to math.js' }, { role: 'assistant', content: [{ type: 'tool_use', id: 'r', name: 'Read', input: { file_path: file } }] }];
  const write = { name: 'Write', input: { file_path: file, content: 'export function sub(a, b) { return a - b; }' } };
  const r = applyPolicy(write, { messages: read, cwd: dir, tools: TOOLS });
  expect(r.call).toEqual({ name: 'Edit', input: { file_path: file, old_string: 'export function mul(a, b) { return a * b; }', new_string: 'export function mul(a, b) { return a * b; }\nexport function sub(a, b) { return a - b; }' } });
  // An Edit whose old_string starts mid-line still loses add(): judged on the file after the edit.
  const midLine = { name: 'Edit', input: { file_path: file, old_string: 'add(a, b) { return a + b; }', new_string: 'sub(a, b) { return a - b; }' } };
  expect(applyPolicy(midLine, { messages: read, cwd: dir, tools: TOOLS }).problem).toMatch(/deletes add\(\)/);
  fs.rmSync(dir, { recursive: true, force: true });
});

test('Jev routing: a confident different step makes ChatJimmy try again with that tool; agreement costs nothing', async () => {
  const routeTo = (choice, p = 0.97) => async () => ({ choice, p, probabilities: {}, latencyMs: 1 });
  // ChatJimmy reaches for Glob; Jev says Grep → asked again with a Grep directive.
  const chat = fakeChat(['{"name": "Glob", "parameters": {"pattern": "**/math.js"}}', '{"name": "Grep", "parameters": {"pattern": "./math.js"}}']);
  const res = await (await post(createAnthropicHandler({ chat, router: routeTo('Grep') }), claudeBody())).json();
  expect(res.content[0]).toMatchObject({ type: 'tool_use', name: 'Grep' });
  expect(chat.calls[1].messages.at(-1).content).toMatch(/use the Grep tool/);
  // Agreement: one ChatJimmy call.
  const agree = fakeChat(['{"name": "Grep", "parameters": {"pattern": "x"}}']);
  await post(createAnthropicHandler({ chat: agree, router: routeTo('Grep') }), claudeBody());
  expect(agree.calls).toHaveLength(1);
  // Low confidence, or a router that failed (null): ChatJimmy's step stands.
  const unsure = fakeChat(['{"name": "Glob", "parameters": {"pattern": "*.md"}}']);
  await post(createAnthropicHandler({ chat: unsure, router: routeTo('Grep', 0.4) }), claudeBody());
  await post(createAnthropicHandler({ chat: unsure, router: async () => null }), claudeBody());
  expect(unsure.calls).toHaveLength(2);
});

test('a call that is one quote or brace short of valid JSON is repaired', () => {
  const r = parseToolCall('{"name": "Edit", "parameters": {"file_path": "package.json", "old_string": "\\"version\\": \\"2.4.1\\"", "new_string": "\\"version\\": \\"2.5.0\\"}}', TOOLS);
  expect(r.call).toEqual({ name: 'Edit', input: { file_path: 'package.json', old_string: '"version": "2.4.1"', new_string: '"version": "2.5.0"' } });
  expect(parseToolCall('{"name": "Read", "parameters": {"file_path": "a.js"', TOOLS).call).toEqual({ name: 'Read', input: { file_path: 'a.js' } });
  expect(parseToolCall('{"name": "Read", "parameters": [[[', TOOLS).problem).toMatch(/not valid JSON/);
});

test('repair: invented enum values are mapped or dropped; a file given as a Grep glob becomes its path', () => {
  const grep = { name: 'Grep', input_schema: { properties: { pattern: { type: 'string' }, path: { type: 'string' }, glob: { type: 'string' }, output_mode: { type: 'string', enum: ['content', 'files_with_matches', 'count'] } }, required: ['pattern'] } };
  expect(repairToolInput('Grep', { pattern: 'x', output_mode: 'list' }, [grep], {}).input.output_mode).toBe('files_with_matches');
  expect(repairToolInput('Grep', { pattern: 'x', output_mode: 'bogus' }, [grep], {}).input).toEqual({ pattern: 'x' });
  expect(repairToolInput('Grep', { pattern: 'x', glob: 'src/math.js' }, [grep], { cwd: WIN }).input).toEqual({ pattern: 'x', path: `${WIN}\\src\\math.js` });
});
