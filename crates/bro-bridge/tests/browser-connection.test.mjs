import { test } from "node:test";
import assert from "node:assert/strict";
import { connectBrowser } from "../src/browser/connection.mjs";

const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
async function until(check) {
  for (let i = 0; i < 200; i++) { if (check()) return; await new Promise(r => setTimeout(r, 5)); }
  assert.fail("Connection did not reach the expected state");
}
class Socket {
  bufferedAmount = 0;
  sent = [];
  send(value) { this.sent.push(JSON.parse(value)); }
  close() { this.closed = true; this.onclose?.(); }
}
function fixture(overrides = {}) {
  const calls = [], messages = [], leases = [], socket = new Socket();
  let sequence = 1;
  const request = async args => {
    calls.push(args);
    if (overrides[args.action]) return overrides[args.action](args);
    if (args.action === "take") return { tabId: 42, lease: "http-lease" };
    if (args.action === "frame") return { title: "Signed-in tab", frame: { sequence: sequence++, data: btoa("jpeg bytes"), metadata: { deviceWidth: 390 } } };
    return {};
  };
  const connection = connectBrowser({
    url: "ws://host/api/browser/stream", session: { tabId: 42, lease: "ws-lease" },
    take: { action: "take", tabId: 42, mobile: true }, request,
    onLease: next => leases.push(next.lease), timeoutMs: 30,
    socketFactory: overrides.socketFactory || (() => socket),
  });
  connection.onmessage = event => messages.push(typeof event.data === "string" ? JSON.parse(event.data) : event.data);
  return { connection, calls, messages, leases, socket };
}

test("WebSocket stays the primary transport and authenticates the exact lease", async t => {
  const f = fixture(); t.after(() => f.connection.close());
  await until(() => f.socket.onopen);
  f.socket.onopen();
  assert.deepEqual(f.socket.sent[0], { tabId: 42, lease: "ws-lease" });
  f.socket.onmessage({ data: new ArrayBuffer(1) });
  await new Promise(r => setTimeout(r, 50));
  assert.equal(f.calls.length, 0);
  assert.equal(f.connection.readyState, 1);
  f.connection.send(JSON.stringify({ type: "ack", sequence: 1 }));
  assert.deepEqual(f.socket.sent[1], { type: "ack", sequence: 1 });
});

test("an upgrade rejected by the network falls back with a fresh exclusive lease", async t => {
  const f = fixture({ socketFactory() { throw Error("Upgrade blocked"); } });
  t.after(() => f.connection.close());
  await until(() => f.messages.some(m => m instanceof ArrayBuffer));
  assert.deepEqual(f.calls.slice(0, 2), [
    { action: "release", tabId: 42, lease: "ws-lease" },
    { action: "take", tabId: 42, mobile: true },
  ]);
  assert.deepEqual(f.leases, ["http-lease"]);
  assert.ok(f.calls.filter(c => c.action === "frame").every(c => c.lease === "http-lease"));
  await new Promise(r => setTimeout(r, 100));
  assert.equal(f.calls.filter(c => c.action === "frame").length, 1, "wait for the displayed image's ACK");
  f.connection.send(JSON.stringify({ type: "ack", sequence: 999 }));
  assert.equal(f.calls.filter(c => c.action === "frame").length, 1);
  f.connection.send(JSON.stringify({ type: "ack", sequence: 1 }));
  await until(() => f.calls.filter(c => c.action === "frame").length === 2);
});

test("a socket that never opens recovers instead of leaving the connecting screen stuck", async t => {
  const f = fixture(); t.after(() => f.connection.close());
  await until(() => f.leases.length === 1);
  assert.equal(f.socket.closed, true);
  assert.equal(f.connection.readyState, 1);
});

test("HTTP Done barrier waits for the last input and preserves its order", async t => {
  const committed = deferred();
  const f = fixture({ socketFactory() { throw Error("blocked"); }, input: () => committed.promise });
  t.after(() => f.connection.close());
  await until(() => f.connection.readyState === 1);
  f.connection.send(JSON.stringify({ type: "input", event: { type: "text", text: "你好 café" } }));
  f.connection.send(JSON.stringify({ type: "flush", id: 7 }));
  await until(() => f.calls.some(c => c.action === "input"));
  assert.equal(f.messages.some(m => m.type === "flushed"), false);
  committed.resolve({});
  await until(() => f.messages.some(m => m.type === "flushed" && m.id === 7));
  assert.equal(f.calls.find(c => c.action === "input").event.text, "你好 café");
  assert.equal(f.calls.some(c => c.resume === true), false);
});

test("closing during fallback releases a late lease and never returns the tab to the agent", async () => {
  const opening = deferred();
  const f = fixture({ socketFactory() { throw Error("blocked"); }, take: () => opening.promise });
  await until(() => f.calls.some(c => c.action === "take"));
  f.connection.close();
  opening.resolve({ tabId: 42, lease: "late-lease" });
  await until(() => f.calls.some(c => c.lease === "late-lease"));
  assert.deepEqual(f.leases, []);
  assert.equal(f.calls.some(c => c.resume === true), false);
  assert.equal(f.connection.readyState, 3);
});

test("failed HTTP input closes the connection without acknowledging Done", async t => {
  const f = fixture({ socketFactory() { throw Error("blocked"); }, input: async () => { throw Error("Host disconnected"); } });
  t.after(() => f.connection.close());
  await until(() => f.connection.readyState === 1);
  f.connection.send(JSON.stringify({ type: "input", event: { type: "text", text: "draft" } }));
  f.connection.send(JSON.stringify({ type: "flush", id: 8 }));
  await until(() => f.connection.readyState === 3);
  assert.ok(f.messages.some(m => m.type === "error" && m.message === "Host disconnected"));
  assert.equal(f.messages.some(m => m.type === "flushed"), false);
  assert.equal(f.calls.some(c => c.resume === true), false);
});

test("HTTP input queues remain bounded when the host stalls", async t => {
  const f = fixture({ socketFactory() { throw Error("blocked"); }, input: () => new Promise(() => {}) });
  t.after(() => f.connection.close());
  await until(() => f.connection.readyState === 1);
  for (let i = 0; i < 33; i++) f.connection.send(JSON.stringify({ type: "input", event: { type: "key", key: "Tab" } }));
  assert.equal(f.connection.readyState, 3);
  assert.ok(f.messages.some(m => m.type === "error" && /catching up/.test(m.message)));
  assert.equal(f.calls.some(c => c.resume === true), false);
});
