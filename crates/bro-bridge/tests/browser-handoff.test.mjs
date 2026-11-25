import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

// Exercise the shipped viewer's event handlers with a small DOM. No browser,
// user tab, or live pairing token is involved in these request-flow tests.
const source = (await readFile(new URL("../src/browser/viewer.js", import.meta.url), "utf8"))
  .replace(/^import .*;$/gm, "")
  .replace(/^export \{.*\};$/gm, "")
  .replace(/^export /gm, "");
class Element {
  hidden = true; disabled = false; open = false; textContent = ""; value = "";
  children = []; dataset = {}; style = { setProperty() {} }; listeners = {};
  setAttribute() {}
  addEventListener(name, fn) { this.listeners[name] = fn; }
  querySelector() { return this.detail ??= new Element(); }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.children = children; }
  showModal() { this.open = true; }
  close() { this.open = false; }
  blur() {}
}
const hiddenRequest = {
  tabId: 42, handoffId: "settings-one", broSessionId: "owner", phase: "requested",
  title: "Browser request", reason: "Clear the host cache in Edge.", viewerAvailable: false,
};
function fixture({ tab = hiddenRequest, query = "?tabId=42&handoffId=settings-one", resume } = {}) {
  const nodes = new Map(), calls = [], messages = [], timers = new Set();
  const element = id => { if (!nodes.has(id)) nodes.set(id, new Element()); return nodes.get(id); };
  const document = { getElementById: element, createElement: () => new Element(), documentElement: new Element(), addEventListener() {} };
  const window = { addEventListener() {}, ipc: { postMessage: value => messages.push(value) }, ReactNativeWebView: { postMessage: value => messages.push(JSON.parse(value).type) } };
  const context = vm.createContext({
    document, window, location: { href: "http://localhost/browser" + query, search: query, hash: "#token=fixture" },
    history: { replaceState() {} }, sessionStorage: { getItem() {}, setItem() {} },
    URL, URLSearchParams, AbortSignal, innerHeight: 800,
    ResizeObserver: class { observe() {} }, WebSocket: { OPEN: 1 },
    setTimeout(fn, ms) { const timer = setTimeout(fn, ms); timer.unref(); timers.add(timer); return timer; },
    clearTimeout,
    createBrowserPointer: () => ({ reset() {}, cancel() {} }), browserPoint() {}, browserGeometry() {},
    connectBrowser() { assert.fail("a protected page must not start a capture connection"); },
    async fetch(_url, options) {
      const args = options.body && JSON.parse(options.body);
      calls.push(args ?? { action: "list" });
      if (!args) return { ok: true, json: async () => ({ tabs: tab ? [tab] : [] }) };
      assert.equal(args.action, "resume", "manual handoff must not take a viewer lease");
      return resume ? resume(args) : { ok: true, json: async () => ({ phase: "agent" }) };
    },
  });
  vm.runInContext(source, context);
  return { element, calls, messages, close: () => { for (const timer of timers) clearTimeout(timer); } };
}
async function until(check) {
  for (let i = 0; i < 100; i++) {
    if (check()) return;
    await new Promise(resolve => setTimeout(resolve, 2));
  }
  assert.fail("Viewer did not reach the expected state");
}

test("a protected-page handoff shows instructions and returns the exact request without capture", async t => {
  const f = fixture(); t.after(f.close);
  await until(() => !f.element("manual-handoff").hidden);
  assert.equal(f.element("manual-reason").textContent, hiddenRequest.reason);
  assert.equal(f.element("done").textContent, "Return control");
  assert.equal(f.element("done").disabled, false);
  assert.equal(f.element("connecting").hidden, true);
  assert.deepEqual(f.calls, [{ action: "list" }]);
  await f.element("done").onclick();
  assert.deepEqual(f.calls[1], { action: "resume", tabId: 42, handoffId: "settings-one" });
  assert.equal(f.element("manual-handoff").hidden, true);
  assert.equal(f.element("status").textContent, "Ready for the agent.");
  assert.deepEqual(f.messages, ["done"]);
});

test("failed Return control keeps the instructions available and can be retried", async t => {
  let attempts = 0;
  const f = fixture({ resume: async () => ++attempts === 1
    ? { ok: false, json: async () => ({ message: "This handoff changed. Refresh Bro and try again." }) }
    : { ok: true, json: async () => ({ phase: "agent" }) } });
  t.after(f.close);
  await until(() => !f.element("manual-handoff").hidden);
  await f.element("done").onclick();
  assert.equal(f.element("manual-handoff").hidden, false);
  assert.equal(f.element("done").disabled, false);
  assert.equal(f.element("retry").textContent, "Refresh request");
  assert.deepEqual(f.messages, []);
  await f.element("done").onclick();
  assert.deepEqual(f.messages, ["done"]);
});

test("leaving the instructions keeps the handoff waiting", async t => {
  const f = fixture(); t.after(f.close);
  await until(() => !f.element("manual-handoff").hidden);
  await f.element("leave").onclick();
  assert.deepEqual(f.calls, [{ action: "list" }]);
  assert.deepEqual(f.messages, ["close"]);
});

test("a stale handoff link opens the picker instead of returning a newer request", async t => {
  const f = fixture({ query: "?tabId=42&handoffId=older" }); t.after(f.close);
  await until(() => f.element("tab-list").children.length > 0);
  assert.equal(f.element("manual-handoff").hidden, true);
  assert.equal(f.element("picker").open, true);
  assert.ok(f.calls.every(call => call.action === "list"));
  await f.element("tab-list").children[0].listeners.click();
  assert.equal(f.element("manual-handoff").hidden, false);
});
