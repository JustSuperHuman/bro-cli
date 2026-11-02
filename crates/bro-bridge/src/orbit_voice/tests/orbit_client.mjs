// Launched by orbit_client.rs. Orbit's real voice server, host client and tool runner talk to
// bro's real host over its protected descriptor. Only the realtime provider is scripted: it plays
// the model, issuing function calls exactly as OpenAI Realtime would. No audio, no paid API.
//   node --import <orbit>/test/fuzz/register.mjs orbit_client.mjs <descriptor> <orbit> <scratch> <expect>
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import http from 'node:http';
import { createRequire } from 'node:module';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [descriptorFile, orbitRoot, scratch, expectJson] = process.argv.slice(2);
const expected = JSON.parse(expectJson);
process.env.USERPROFILE = process.env.HOME = path.join(scratch, 'home');
process.env.APPDATA = path.join(scratch, 'appdata');
process.env.LOCALAPPDATA = path.join(scratch, 'local');
globalThis.__fuzz = { rng: () => 0, str: () => 'x', calls: [], windowTitle: () => 'Never automate Claude' };
const orbit = (relative) => import(pathToFileURL(path.join(orbitRoot, relative)).href);
const { createExternalHost, readDescriptor, selectExternalHost } = await orbit('src/external-host.js');
const config = await orbit('src/config.js');
const server = await orbit('src/server.js');
const { WebSocket } = createRequire(path.join(orbitRoot, 'package.json'))('ws');
const log = (...args) => console.error('[orbit-driver]', ...args);

const descriptor = readDescriptor(descriptorFile); // real ACL and shape validation
const client = createExternalHost({ descriptor, pollMs: 150 });
await client.start();
selectExternalHost(client);
log('host capabilities', client.snapshot().capabilities.join(', '));
log('catalog', client.catalog()?.revision, (client.catalog()?.tools || []).map((t) => t.name).join(', '));
assert.ok(client.catalog()?.tools.length >= 7, 'the host tool catalog was accepted');

// The scripted model: each response.create consumes the next scripted turn of function calls.
const turns = [];
const sent = [];
let provider, responses = 0;
class Provider extends EventEmitter {
  readyState = 1;
  send(raw) {
    const message = JSON.parse(raw);
    sent.push(message);
    if (message.type === 'response.create') setImmediate(respond);
  }
  event(evt) { this.emit('message', Buffer.from(JSON.stringify(evt)), false); }
  close() { if (this.readyState !== 3) { this.readyState = 3; this.emit('close', 1000, Buffer.from('fixture')); } }
}
function respond() {
  const id = `response-${++responses}`;
  const calls = turns.shift() || [];
  provider.event({ type: 'response.created', response: { id } });
  for (const call of calls) provider.event({ type: 'response.function_call_arguments.done', response_id: id, call_id: call.id, name: call.name, arguments: JSON.stringify(call.args) });
  provider.event({ type: 'response.done', response: { id, status: 'completed' } });
}

const mem = (value = '') => ({ kind: 'memory', get: () => value, set: (v) => { value = v; } });
const origin = await server.startServer({ port: 0, configDir: path.join(scratch, 'cfg'), importEnv: false, secrets: mem(), brainSecrets: mem(),
  openaiSecrets: mem('sk-fixture-openai-secret-not-live-0000'),
  realtimeFactory: () => { provider = new Provider(); setImmediate(() => provider.emit('open')); return provider; } });
config.updateSettings({ voiceEngine: 'openai', brainEnabled: false, announce: 'off' });
const cookie = await new Promise((resolve) => http.get(new URL(server.authUrl()), (res) => { res.resume(); resolve(res.headers['set-cookie'][0].split(';')[0]); }));
const ws = new WebSocket(`${origin.replace('http:', 'ws:')}/voice`, { headers: { origin, cookie } });
const events = [];
ws.on('message', (bytes) => events.push(JSON.parse(bytes)));
const waitFor = async (what, predicate, ms = 15000) => {
  for (const until = Date.now() + ms; Date.now() < until;) { const value = predicate(); if (value) return value; await new Promise((r) => setTimeout(r, 10)); }
  throw new Error(`Timed out waiting for ${what}`);
};
const output = (callId) => {
  const item = sent.find((m) => m.type === 'conversation.item.create' && m.item?.type === 'function_call_output' && m.item.call_id === callId);
  return item && JSON.parse(item.item.output);
};
const say = async (text, calls) => {
  turns.push(calls);
  ws.send(JSON.stringify({ type: 'ui.text', text }));
  return Object.fromEntries(await Promise.all(calls.map(async (c) => [c.id, await waitFor(`output of ${c.name} (${c.id})`, () => output(c.id))])));
};

await waitFor('provider session', () => events.some((e) => e.type === 'bridge.ready'));
const session = await waitFor('session.update with host tools', () => [...sent].reverse().find((m) => m.type === 'session.update' && m.session?.tools?.some((t) => t.name === 'read_session')));
const offered = session.session.tools.map((t) => t.name);
log('tools offered to the model', offered.join(', '));
for (const name of (client.catalog()?.tools || []).map((t) => t.name)) assert.ok(offered.includes(name), `catalog tool ${name} reaches the model`);

// Turn 1: reads. Discovery, scoped scrollback, a refused foreign read, chats and task status.
const reads = await say('What is the sibling tab doing? Check my chats and tasks too.', [
  { id: 'c-sessions', name: 'list_sessions', args: {} },
  { id: 'c-projects', name: 'list_projects', args: {} },
  { id: 'c-read', name: 'read_session', args: { sessionId: 'sibling', lines: 20 } },
  { id: 'c-foreign', name: 'read_session', args: { sessionId: 'foreign', lines: 20 } },
  { id: 'c-chats', name: 'list_chats', args: {} },
  { id: 'c-chat', name: 'read_chat', args: { chatId: expected.chatId, messages: 5 } },
  { id: 'c-tasks', name: 'coordination_snapshot', args: {} },
  { id: 'c-unknown', name: 'send_input', args: { sessionId: 'sibling', text: 'rm -rf /' } },
]);
for (const [id, value] of Object.entries(reads)) log(id, JSON.stringify(value).slice(0, 300));
assert.deepEqual(reads['c-sessions'].sessions.map((s) => s.id).sort(), ['foreign', 'sibling', 'source']);
assert.equal(reads['c-sessions'].sessions.find((s) => s.id === 'sibling').inScope, true);
assert.equal(reads['c-sessions'].sessions.find((s) => s.id === 'foreign').inScope, false);
assert.ok(Array.isArray(reads['c-projects'].projects));
assert.match(reads['c-read'].text, new RegExp(expected.sibling));
assert.equal(reads['c-read'].nonUser, true);
assert.match(reads['c-foreign'].error, /declined: .*project/, 'a session in another project is refused with the host reason');
assert.equal(reads['c-foreign'].code, 'outside_scope');
assert.ok(!JSON.stringify(reads['c-foreign']).includes(expected.foreign));
assert.ok(reads['c-chats'].chats.some((c) => c.chatId === expected.chatId), 'planted project chat is listed');
assert.match(reads['c-chat'].text, /schema 1/);
assert.ok(Array.isArray(reads['c-tasks'].tasks));
assert.ok(reads['c-unknown'].error, 'tools outside the catalog are not executed');

// Turn 2: delegated work. Draft from the user's words, sent by the same utterance's instruction.
const work = await say('Run the unit tests and send it', [{ id: 'c-draft', name: 'draft_prompt', args: { text: 'Run the unit tests' } }]);
log('c-draft', JSON.stringify(work['c-draft']));
assert.equal(work['c-draft'].state, 'queued', 'the reply reports durable admission, not bare receipt');
assert.equal(work['c-draft'].durablyAccepted, true, JSON.stringify(work['c-draft']));
const requestId = work['c-draft'].requestId;

// bro finishes the request; the correlated result reaches the model as non-user tool output.
const delivered = await waitFor('host result delivered to the model', () => sent.find((m) => m.item?.type === 'function_call_output' && m.item.output.includes(expected.result)));
const result = JSON.parse(delivered.item.output);
log('host result', JSON.stringify(result));
assert.equal(result.requestId, requestId);
assert.equal(result.nonUser, true);

// Turn 3: the model checks the request it knows about.
const status = await say('Did that finish?', [{ id: 'c-status', name: 'request_status', args: { requestId } }]);
log('c-status', JSON.stringify(status['c-status']));
assert.equal(status['c-status'].state, 'completed');
assert.ok(!JSON.stringify(events).includes(descriptor.token), 'the pairing token never reaches the renderer');

console.log(`RESULT ${JSON.stringify({ offered, submittedRequestId: requestId, reads: Object.keys(reads), status: status['c-status'].state })}`);
ws.close();
client.stop();
await server.closeServer();
process.exit(0);
