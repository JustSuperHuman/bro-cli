// Real extension + isolated paired bridge + a recorded agent sink; never sends to a user's session.
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
const info = JSON.parse(await readFile(process.argv[2], "utf8"));
assert.ok(info.fixtureSessionId?.startsWith("browser-wake-fixture-"), "Start browser_live with --wake-test");
const results = [];
let session, counter = 0, tab, lease;
const sessions = new Set();
async function rpc(method, params) {
  const response = await fetch("http://127.0.0.1:12306/mcp", { method: "POST", headers: {
    "Content-Type": "application/json", Accept: "application/json, text/event-stream",
    "x-bro-session-id": info.fixtureSessionId, ...(session ? { "mcp-session-id": session } : {}),
  }, body: JSON.stringify({ jsonrpc: "2.0", id: ++counter, method, params }) });
  session = response.headers.get("mcp-session-id") || session;
  if (session) sessions.add(session);
  const text = await response.text();
  assert.equal(response.status, 200, text.slice(0, 200));
  const packet = text.trim().startsWith("{") ? JSON.parse(text) : text.split("\n").filter(line => line.startsWith("data:")).map(line => JSON.parse(line.slice(5))).find(value => value.id === counter);
  if (packet.error) throw Error(packet.error.message);
  return packet.result;
}
async function tool(name, args) {
  const result = await rpc("tools/call", { name, arguments: args });
  const text = result.content?.find(item => item.type === "text")?.text;
  if (result.isError) throw Error(text);
  try { return JSON.parse(text); } catch { return text; }
}
async function api(args) {
  const response = await fetch(info.url + "/api/browser" + (args ? "/command" : ""), { method: args ? "POST" : "GET", headers: {
    Authorization: `Bearer ${info.token}`, ...(args ? { "Content-Type": "application/json" } : {}),
  }, ...(args ? { body: JSON.stringify(args) } : {}) });
  const result = await response.json(); assert.equal(response.status, 200, result.message); return result;
}
async function wakes() { return (await readFile(info.wakeLog, "utf8")).trim().split("\n").filter(Boolean).map(line => JSON.parse(line)); }
async function until(check, label, timeout = 15_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await check()) return; await new Promise(resolve => setTimeout(resolve, 100)); }
  throw Error(`Timed out: ${label}`);
}
async function activity(value) {
  await writeFile(info.activityFile, value);
  await until(async () => {
    const response = await fetch(info.url + "/api/bootstrap", { headers: { Authorization: `Bearer ${info.token}` } });
    const body = await response.json(); return body.sessions.find(s => s.id === info.fixtureSessionId)?.agentActivity === value;
  }, `fixture agent becomes ${value}`);
}
function passed(label) { results.push(label); console.log("PASS", label); }
try {
  await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "bro-handoff-wake-test", version: "1" } });
  await fetch("http://127.0.0.1:12306/mcp", { method: "POST", headers: { "Content-Type": "application/json", Accept: "application/json, text/event-stream", "mcp-session-id": session }, body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) });
  const opened = await tool("chrome_navigate", { url: "about:blank", background: true }); tab = opened.tabId;
  await tool("chrome_javascript", { tabId: tab, code: "document.title='Bro handoff wake fixture';document.body.innerHTML='<input id=draft value=original>';draft.focus();return true;" });
  const requested = await tool("chrome_request_handoff", { tabId: tab, reason: "Complete the wake-up fixture.", broSessionId: info.fixtureSessionId });
  assert.ok(requested.handoffId);
  assert.equal(requested.broSessionId, info.fixtureSessionId);
  let controlled = await api({ action: "take", tabId: tab }); lease = controlled.lease;
  await api({ action: "release", tabId: tab, lease });
  await new Promise(resolve => setTimeout(resolve, 6000));
  assert.equal((await wakes()).length, 0);
  passed("disconnect leaves the agent waiting without a wake-up");
  controlled = await api({ action: "take", tabId: tab }); lease = controlled.lease;
  assert.equal(controlled.handoffId, requested.handoffId);
  await api({ action: "input", tabId: tab, lease, event: { type: "text", text: " confirmed" } });
  await api({ action: "release", tabId: tab, lease, resume: true });
  await until(async () => (await wakes()).length === 1, "idle owner receives its completion");
  const first = (await wakes())[0];
  assert.equal(first.sessionId, info.fixtureSessionId);
  assert.equal(first.eventId, requested.handoffId);
  assert.ok(first.text.includes(String(tab)) && first.text.includes("original task"));
  passed("Done wakes the exact idle owner with the same tab and task context");
  await api({ action: "release", tabId: tab, lease, resume: true });
  await new Promise(resolve => setTimeout(resolve, 6000));
  assert.equal((await wakes()).length, 1);
  passed("repeated Done and subsequent polls cannot deliver another turn");
  const state = await tool("chrome_javascript", { tabId: tab, code: "return draft.value;" });
  assert.ok(JSON.stringify(state).includes("confirmed"));
  await activity("working");
  await tool("chrome_request_handoff", { tabId: tab, reason: "The active agent will recheck this handoff." });
  controlled = await api({ action: "take", tabId: tab }); lease = controlled.lease;
  await api({ action: "release", tabId: tab, lease, resume: true });
  await tool("chrome_javascript", { tabId: tab, code: "return draft.value;" });
  await activity("idle");
  await new Promise(resolve => setTimeout(resolve, 6000));
  assert.equal((await wakes()).length, 1);
  passed("an active agent's actual page recheck suppresses a redundant wake-up");
  await tool("chrome_request_handoff", { tabId: tab, reason: "Recover this handoff after the MCP connection changes." });
  controlled = await api({ action: "take", tabId: tab }); lease = controlled.lease;
  await api({ action: "release", tabId: tab, lease, resume: true });
  await until(async () => (await wakes()).length === 2, "next completion wakes its owner");
  session = undefined;
  await rpc("initialize", { protocolVersion: "2025-03-26", capabilities: {}, clientInfo: { name: "bro-handoff-reconnected", version: "1" } });
  await fetch("http://127.0.0.1:12306/mcp", { method: "POST", headers: { "Content-Type": "application/json", Accept: "application/json, text/event-stream", "mcp-session-id": session }, body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) });
  const returned = await tool("chrome_wait_for_handoff", { tabId: tab, timeoutMs: 0 });
  assert.equal(returned.phase, "agent");
  const recovered = await tool("chrome_javascript", { tabId: tab, code: "return draft.value;" });
  assert.ok(JSON.stringify(recovered).includes("confirmed"));
  passed("a reconnected MCP transport of the same Bro session can inspect the returned tab");
  const desktop = await tool("chrome_request_handoff", { tabId: tab, reason: "I finished in my signed-in desktop browser." });
  await api({ action: "resume", tabId: tab, handoffId: desktop.handoffId });
  await until(async () => (await wakes()).length === 3, "desktop Done wakes the agent without a viewer");
  assert.equal((await tool("chrome_wait_for_handoff", { tabId: tab, timeoutMs: 0 })).phase, "agent");
  assert.ok(JSON.stringify(await tool("chrome_javascript", { tabId: tab, code: "return draft.value;" })).includes("confirmed"));
  passed("desktop Return control completes the same signed-in tab without connecting a viewer");
  const next = await tool("chrome_request_handoff", { tabId: tab, reason: "A new desktop interaction is required." });
  await assert.rejects(api({ action: "resume", tabId: tab, handoffId: desktop.handoffId }));
  assert.equal((await tool("chrome_wait_for_handoff", { tabId: tab, timeoutMs: 0 })).phase, "requested");
  controlled = await api({ action: "take", tabId: tab }); lease = controlled.lease;
  await assert.rejects(api({ action: "resume", tabId: tab, handoffId: next.handoffId }));
  await api({ action: "release", tabId: tab, lease });
  await api({ action: "resume", tabId: tab, handoffId: next.handoffId });
  passed("stale desktop buttons and an active viewer cannot release the wrong handoff");
  if (process.argv.includes("--desktop-ui")) {
    await tool("chrome_javascript", { tabId: tab, code: "document.title='Bro desktop handoff fixture';return draft.value;" });
    const progress = process.argv[2] + ".desktop.json";
    for (const stage of ["return", "viewer"]) {
      const requested = await tool("chrome_request_handoff", { tabId: tab, reason: stage === "return" ? "The page is ready. Return control from the desktop workspace." : "Open this exact tab from the desktop workspace, then use Done." });
      await writeFile(progress, JSON.stringify({ stage, tabId: tab, handoffId: requested.handoffId }));
      console.log("WAIT desktop", stage);
      await until(async () => (await tool("chrome_wait_for_handoff", { tabId: tab, timeoutMs: 0 })).phase === "agent", `desktop UI ${stage}`, 1_200_000);
      const page = await tool("chrome_javascript", { tabId: tab, code: "return {title:document.title,draft:draft.value};" });
      assert.ok(JSON.stringify(page).includes("Bro desktop handoff fixture") && JSON.stringify(page).includes("confirmed"));
      passed(stage === "return" ? "the native workspace Return control button releases the exact handoff" : "the native workspace opens the exact tab and its viewer Done returns control");
    }
    await writeFile(progress, JSON.stringify({ stage: "complete", tabId: tab }));
  }
  await writeFile(process.argv[2] + ".results.json", JSON.stringify({ passed: results, verifiedAt: new Date().toISOString() }, null, 2));
} finally {
  if (tab) {
    if (lease) await api({ action: "release", tabId: tab, lease, resume: true }).catch(() => {});
    await tool("chrome_close_tabs", { tabIds: [tab] }).catch(() => {});
  }
  for (const id of sessions) await fetch("http://127.0.0.1:12306/mcp", { method: "DELETE", headers: { "mcp-session-id": id } }).catch(() => {});
}
