// Live acceptance test: real signed-in host browser + isolated paired gateway + fresh mobile/desktop viewers.
// Start examples/browser_live.rs first. Pass --info <connection.json> and --playwright <playwright-core directory>.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { resolve } from "node:path";
const argument = (key, fallback) => {
  const at = process.argv.indexOf(key);
  return at < 0 ? fallback : process.argv[at + 1];
};
const connection = JSON.parse(await readFile(argument("--info"), "utf8"));
const playwright = createRequire(import.meta.url)(
  argument("--playwright", "playwright"),
);
const engine = argument("--engine", "chromium");
const legacySession = argument("--legacy", "false") === "true";
assert.ok(["chromium", "webkit"].includes(engine), "Use chromium or webkit");
const report = resolve(argument("--out", ".impeccable/review/browser"));
await mkdir(report, { recursive: true });
const results = [],
  events = [];
let finishManual;
const manualComplete = new Promise((resolve) => {
  finishManual = resolve;
});
const cookie = randomUUID();
const cookieName = `broBrowserProof_${randomUUID()}`;
const broSessionId = `browser-fixture-${randomUUID()}`;
const fixture = createServer(async (request, response) => {
  if (request.url === "/cleanup") {
    response.setHeader(
      "Set-Cookie",
      [cookieName, "broBrowserProof"].map(
        (name) => `${name}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0`,
      ),
    );
    response.end("cleaned");
    return;
  }
  if (request.url === "/finish") {
    response.end("finished");
    finishManual();
    return;
  }
  if (request.url === "/event") {
    let body = "";
    for await (const part of request) body += part;
    events.push(JSON.parse(body));
    response.end("ok");
    return;
  }
  if (request.url === "/auth") {
    response.setHeader("Content-Type", "application/json");
    response.end(
      JSON.stringify({
        signedIn: (request.headers.cookie || "").includes(
          `${cookieName}=${cookie}`,
        ),
      }),
    );
    return;
  }
  response.setHeader("Content-Type", "text/html; charset=utf-8");
  response.setHeader(
    "Set-Cookie",
    `${cookieName}=${cookie}; HttpOnly; SameSite=Lax; Path=/`,
  );
  response.end(`<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><title>Bro handoff fixture</title>
    <style>body{margin:0;padding:24px;font:16px system-ui;color:#24282e;background:#fff}h1{font-size:24px;margin:0 0 10px}p{line-height:1.5}input,button{font:inherit;box-sizing:border-box;min-height:48px;padding:12px;border:1px solid #89929e;border-radius:6px}input{display:block;width:min(100%,600px);margin:12px 0}button{background:#dce4ee}#large{height:1300px;padding-top:24px}</style>
    <h1>Keep the conversation moving.</h1><p>Signed in on this browser. The same tab stays open.</p>
    <label>Unsent draft<input id="draft" value="Original draft"></label><button id="increment">Confirm selection</button><p id="count">Waiting for your choice</p><div id="large">Drag to scroll this page.</div>
    <script>window.instanceNonce=crypto.randomUUID();window.confirmed=0;localStorage.setItem('bro-browser-proof','local');sessionStorage.setItem('bro-browser-proof','session');const tell=(type,value)=>fetch('/event',{method:'POST',body:JSON.stringify({type,value})});draft.addEventListener('input',()=>tell('input',draft.value));increment.onclick=()=>{confirmed++;count.textContent='Selection confirmed '+confirmed;tell('confirm',confirmed)};document.addEventListener('scroll',()=>tell('scroll',scrollY));</script>`);
});
await new Promise((r) => fixture.listen(0, "127.0.0.1", r));
const fixtureURL = `http://127.0.0.1:${fixture.address().port}`;
let mcpSession,
  requestID = 0,
  tabId,
  browser,
  controllerLease;
async function rpc(method, params) {
  const response = await fetch("http://127.0.0.1:12306/mcp", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      ...(!legacySession ? { "x-bro-session-id": broSessionId } : {}),
      ...(mcpSession ? { "mcp-session-id": mcpSession } : {}),
    },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++requestID, method, params }),
  });
  if (response.headers.get("mcp-session-id"))
    mcpSession = response.headers.get("mcp-session-id");
  const text = await response.text();
  assert.equal(response.status, 200, text.slice(0, 200));
  const message = text.trim().startsWith("{")
    ? JSON.parse(text)
    : text
        .split("\n")
        .filter((l) => l.startsWith("data:"))
        .map((l) => JSON.parse(l.slice(5)))
        .find((v) => v.id === requestID);
  if (message.error) throw new Error(message.error.message);
  return message.result;
}
async function tool(name, args = {}, allowError = false) {
  const result = await rpc("tools/call", { name, arguments: args });
  if (allowError) return result;
  assert.ok(!result.isError, JSON.stringify(result));
  const value = result.content?.find((c) => c.type === "text")?.text;
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}
async function js(code) {
  const value = await tool("chrome_javascript", { tabId, code });
  assert.equal(value.success, true, JSON.stringify(value));
  try {
    return JSON.parse(value.result);
  } catch {
    return value.result;
  }
}
async function api(args, status = 200) {
  const response = await fetch(
    connection.url + "/api/browser" + (args ? "/command" : ""),
    {
      method: args ? "POST" : "GET",
      headers: {
        Authorization: `Bearer ${connection.token}`,
        ...(args ? { "Content-Type": "application/json" } : {}),
      },
      ...(args ? { body: JSON.stringify(args) } : {}),
    },
  );
  const body = await response.json();
  assert.equal(response.status, status, body.message);
  return body;
}
async function until(test, label, timeout = 12_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await test()) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`Timed out: ${label}`);
}
function passed(label) {
  results.push(label);
  console.log("PASS", label);
}
try {
  await until(
    async () => {
      try {
        return (await fetch("http://127.0.0.1:12306/ping")).ok;
      } catch {
        return false;
      }
    },
    "connected JustBrowse extension",
    25_000,
  );
  await rpc("initialize", {
    protocolVersion: "2025-03-26",
    capabilities: {},
    clientInfo: { name: "bro-browser-test", version: "1" },
  });
  const response = await fetch("http://127.0.0.1:12306/mcp", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      "mcp-session-id": mcpSession,
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      method: "notifications/initialized",
    }),
  });
  assert.ok(response.ok);
  const opened = await tool("chrome_navigate", {
    url: fixtureURL,
    background: true,
  });
  tabId = opened.tabId;
  assert.ok(tabId, JSON.stringify(opened));
  const before = await js(
    "draft.value='Unsent draft'; return {pageIdentity:instanceNonce,localValue:localStorage.getItem('bro-browser-proof'),storageValue:sessionStorage.getItem('bro-browser-proof'),signedIn:(await (await fetch('/auth')).json()).signedIn};",
  );
  assert.equal(before.signedIn, true);
  const handoff = await tool("chrome_request_handoff", {
    tabId,
    reason: "Confirm the selection, then continue.",
  });
  assert.equal(handoff.phase, "requested");
  assert.equal(await js("return draft.value;"), "Unsent draft");
  passed("request lets the agent inspect the page before Done");
  assert.equal(
    (await tool("__justbrowse_remote", { action: "list" }, true)).isError,
    true,
  );
  passed("agents cannot call the internal paired-viewer commands");
  browser = await playwright[engine].launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 3,
    isMobile: true,
    hasTouch: true,
    colorScheme: "dark",
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  let meta, activeLease;
  page.on("websocket", (ws) => {
    ws.on("framesent", (event) => {
      try {
        const v = JSON.parse(String(event.payload));
        if (v.lease) activeLease = controllerLease = v.lease;
      } catch {}
    });
    ws.on("framereceived", (event) => {
      try {
        const v = JSON.parse(String(event.payload));
        if (v.type === "frame") meta = v.frame.metadata;
      } catch {}
    });
  });
  await page.goto(
    `${connection.url}/browser?sessionId=${broSessionId}#token=${connection.token}`,
  );
  await page.getByRole("button", { name: "Bro handoff fixture" }).waitFor();
  if (legacySession) {
    const listed = (await api()).tabs.find(tab => tab.tabId === tabId);
    assert.equal(listed.broSessionId, undefined);
    passed("waiting legacy handoff remains selectable inside a chat-scoped picker");
  }
  await page.screenshot({ path: resolve(report, "mobile-picker.png") });
  await page.getByRole("button", { name: "Bro handoff fixture" }).click();
  await page.locator("#frame:not([hidden])").waitFor({ timeout: 30_000 });
  await until(() => !!meta && !!activeLease, "first live frame");
  const remoteViewport = { width: meta.deviceWidth, height: meta.deviceHeight };
  // The continuity cases below exercise the optional direct-touch mode.
  await page.locator("#pointer-mode").click();
  passed("mobile viewer receives the existing signed-in tab at DPR 3");
  await api({ action: "take", tabId }, 409);
  passed("second controller is rejected");
  const addresses = await page.evaluate(() => {
    const values = ["example.com", "weather tomorrow"],
      sent = [],
      validity = [];
    const original = WebSocket.prototype.send;
    const address = document.querySelector("#address"),
      saved = address.value;
    WebSocket.prototype.send = function (data) {
      const message = typeof data === "string" ? JSON.parse(data) : {};
      if (message.type === "input" && message.event.type === "navigate") {
        sent.push(message.event.url);
        return; // Verify form submission without leaving the continuity fixture.
      }
      return original.call(this, data);
    };
    try {
      for (const value of values) {
        address.value = value;
        validity.push(address.checkValidity());
        document.querySelector("#address-form").requestSubmit();
        const next = document.querySelector("#new-address");
        next.value = value;
        validity.push(next.checkValidity());
      }
    } finally {
      WebSocket.prototype.send = original;
      address.value = saved;
      document.querySelector("#new-address").value = "";
    }
    return { sent, validity };
  });
  assert.deepEqual(addresses, {
    sent: [
      "https://example.com",
      "https://www.google.com/search?q=weather%20tomorrow",
    ],
    validity: [true, true, true, true],
  });
  passed(
    "address forms accept bare domains and searches and dispatch normalized navigation",
  );
  await page.locator("#reload").focus();
  const beforeFocus = await page.screenshot();
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => document.activeElement.id), "stage");
  const withFocus = await page.screenshot({
    path: resolve(report, "mobile-focus.png"),
  });
  const focusBox = await page.locator("#stage").boundingBox();
  const focusPixels = await page.evaluate(
    async ({ sources, box }) => {
      const samples = [];
      for (const source of sources) {
        const shot = new Image();
        shot.src = `data:image/png;base64,${source}`;
        await shot.decode();
        const canvas = document.createElement("canvas");
        canvas.width = shot.naturalWidth;
        canvas.height = shot.naturalHeight;
        const context = canvas.getContext("2d");
        context.drawImage(shot, 0, 0);
        samples.push(
          [1, 3].map((offset) =>
            [
              ...context.getImageData(
                Math.round((box.x + offset) * devicePixelRatio),
                Math.round((box.y + box.height / 2) * devicePixelRatio),
                1,
                1,
              ).data,
            ].slice(0, 3),
          ),
        );
      }
      return samples;
    },
    {
      sources: [beforeFocus.toString("base64"), withFocus.toString("base64")],
      box: focusBox,
    },
  );
  assert.ok(
    focusPixels[1][0].every((c) => c < 40),
    JSON.stringify(focusPixels),
  );
  assert.ok(
    focusPixels[1][1].every((c) => c > 230),
    JSON.stringify(focusPixels),
  );
  assert.ok(
    focusPixels[1].some((pixel, i) =>
      pixel.some((c, j) => Math.abs(c - focusPixels[0][i][j]) > 150),
    ),
    "focus must change painted pixels above the image",
  );
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => document.activeElement.id), "reload");
  assert.equal(
    await page.evaluate(() =>
      [...document.querySelectorAll("button,input[type=text]")]
        .filter(
          (e) =>
            e.getBoundingClientRect().width && e.getBoundingClientRect().height,
        )
        .every((e) => {
          const r = e.getBoundingClientRect();
          return r.width >= 48 && r.height >= 48;
        }),
    ),
    true,
  );
  passed(
    "remote-page keyboard focus is visible and escapable; controls meet the 48px touch floor",
  );
  const viewerPoint = async (x, y) => {
    const box = await page.locator("#frame").boundingBox();
    const scale = box.width / meta.deviceWidth;
    return {
      x: box.x + x * scale,
      y: box.y + (y + (meta.offsetTop || 0)) * scale,
    };
  };
  // The remote page keeps the browser's viewport while this viewer scales it locally.
  let p = await viewerPoint(120, 183);
  await page.touchscreen.tap(p.x, p.y);
  await page.locator("#keyboard").click();
  await page.locator("#keyboard-input").fill(" + mobile café 👋");
  await until(
    () =>
      events.some((e) => e.type === "input" && e.value.includes("mobile café")),
    "committed Unicode input",
  );
  await page.screenshot({ path: resolve(report, "mobile-keyboard.png") });
  await page.locator("#hide-keyboard").click();
  p = await viewerPoint(100, 243);
  await page.touchscreen.tap(p.x, p.y);
  await until(
    () => events.some((e) => e.type === "confirm"),
    "remote click confirmation",
  );
  passed("touch coordinates and Unicode keyboard reach the remote page");
  await new Promise((r) => setTimeout(r, 1200));
  await page.screenshot({ path: resolve(report, "mobile.png") });
  const mapping = await page.evaluate(async () => {
    const { browserPoint } = await import("/browser/viewer.js");
    return [
      browserPoint(
        {
          left: 100,
          top: 50,
          right: 300,
          bottom: 450,
          width: 200,
          height: 400,
        },
        { deviceWidth: 400, offsetTop: 20 },
        200,
        250,
      ),
      browserPoint(
        {
          left: 100,
          top: 50,
          right: 300,
          bottom: 450,
          width: 200,
          height: 400,
        },
        { deviceWidth: 400, offsetTop: 20 },
        50,
        250,
      ),
    ];
  });
  assert.deepEqual(mapping, [{ x: 200, y: 380 }, null]);
  passed("letterboxing and scaled-frame geometry stay in CSS pixels");
  await page.setViewportSize({ width: 320, height: 740 });
  assert.equal(
    await page.evaluate(
      () => document.documentElement.scrollWidth > innerWidth,
    ),
    false,
  );
  await page.screenshot({ path: resolve(report, "mobile-small.png") });
  await page.setViewportSize({ width: 844, height: 390 });
  assert.deepEqual({ width: meta.deviceWidth, height: meta.deviceHeight }, remoteViewport);
  await until(
    () =>
      page.locator("#frame").evaluate((e) => {
        const r = e.getBoundingClientRect();
        const stage = document.querySelector("#stage"),
          style = getComputedStyle(stage);
        const width =
          stage.clientWidth -
          parseFloat(style.paddingLeft) -
          parseFloat(style.paddingRight);
        const height =
          stage.clientHeight -
          parseFloat(style.paddingTop) -
          parseFloat(style.paddingBottom);
        const expected = Math.min(
          width,
          (height * e.naturalWidth) / e.naturalHeight,
        );
        return (
          Math.abs(r.width - expected) < 2
        );
      }),
    "landscape viewer fits the unchanged remote viewport",
  ).catch(async (error) => {
    const geometry = await page.locator("#frame").evaluate((e) => ({
      naturalWidth: e.naturalWidth,
      naturalHeight: e.naturalHeight,
      rect: e.getBoundingClientRect().toJSON(),
      stage: document.querySelector("#stage").getBoundingClientRect().toJSON(),
    }));
    await page.screenshot({ path: resolve(report, "mobile-landscape.png") });
    throw new Error(`${error.message}: ${JSON.stringify({ meta, geometry })}`);
  });
  await page.screenshot({ path: resolve(report, "mobile-landscape.png") });
  await page.setViewportSize({ width: 390, height: 844 });
  const oldLease = activeLease;
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await until(
    async () =>
      (await api()).tabs.find((t) => t.tabId === tabId)?.phase === "requested",
    "disconnect releases controller and keeps the request visible",
  );
  await page.screenshot({ path: resolve(report, "mobile-error.png") });
  assert.ok(!(await tool("chrome_read_page", { tabId }, true)).isError);
  await page.locator("#retry").click();
  await page.locator("#frame:not([hidden])").waitFor({ timeout: 30_000 });
  await until(() => activeLease !== oldLease, "fresh reconnect lease");
  await api(
    {
      action: "input",
      tabId,
      lease: oldLease,
      event: { type: "text", text: "stale" },
    },
    409,
  );
  passed("disconnect permits agent inspection; reconnect rejects the old lease");
  p = await viewerPoint(120, 183);
  await page.touchscreen.tap(p.x, p.y);
  await page.locator("#keyboard").click();
  // Commit text and tap Done in the same task: no frame or arbitrary delay can
  // accidentally provide the ordering guarantee that the input barrier owes.
  await page.evaluate(() => {
    const keyboard = document.querySelector("#keyboard-input");
    keyboard.value = " + immediate-Done";
    keyboard.dispatchEvent(
      new InputEvent("input", {
        bubbles: true,
        inputType: "insertText",
        data: keyboard.value,
      }),
    );
    document.querySelector("#done").click();
  });
  await until(
    async () =>
      (await tool("chrome_wait_for_handoff", { tabId, timeoutMs: 0 })).phase ===
      "agent",
    "explicit handback",
  );
  const after = await js(
    "return {pageIdentity:instanceNonce,localValue:localStorage.getItem('bro-browser-proof'),storageValue:sessionStorage.getItem('bro-browser-proof'),draft:draft.value,confirmed,signedIn:(await (await fetch('/auth')).json()).signedIn};",
  );
  assert.equal(after.pageIdentity, before.pageIdentity);
  assert.equal(after.localValue, before.localValue);
  assert.equal(after.storageValue, before.storageValue);
  assert.equal(after.signedIn, true);
  assert.ok(after.draft.includes("mobile café"));
  assert.ok(after.draft.includes("immediate-Done"), after.draft);
  passed("Done waits for the last input committed in the same event-loop task");
  assert.equal(after.confirmed, 1);
  passed(
    "Done restores agent control with HttpOnly auth, storage, draft and live JS state intact",
  );
  await page.setViewportSize({ width: 1100, height: 800 });
  await tool("chrome_request_handoff", { tabId, reason: "Continue the same tab in the desktop viewer." });
  await page.locator("#tabs").click();
  await page.getByRole("button", { name: "Bro handoff fixture" }).click();
  await page.locator("#frame:not([hidden])").waitFor({ timeout: 30_000 });
  await new Promise((r) => setTimeout(r, 1000));
  await page.screenshot({ path: resolve(report, "desktop.png") });
  await page.locator("#done").click();
  assert.deepEqual(errors, []);
  passed(
    "shared desktop viewer renders and releases without JavaScript errors",
  );
  await until(
    async () =>
      (await tool("chrome_wait_for_handoff", { tabId, timeoutMs: 0 })).phase ===
      "agent",
    "desktop Done completes before closing viewer",
  );
  await context.close();
  const rawLease = await api({
    action: "take",
    tabId,
    mobile: true,
    width: 390,
    height: 740,
    pixelRatio: 2,
  });
  controllerLease = rawLease.lease;
  const raw = new WebSocket(
    `${connection.url.replace("http://", "ws://")}/api/browser/stream?token=${encodeURIComponent(connection.token)}`,
  );
  let binaryFrames = 0,
    ackSequence,
    frameBytes = 0;
  raw.binaryType = "arraybuffer";
  raw.onopen = () => raw.send(JSON.stringify({ tabId, lease: rawLease.lease }));
  raw.onmessage = (event) => {
    if (typeof event.data === "string") {
      const value = JSON.parse(event.data);
      if (value.type === "frame") ackSequence = value.frame.sequence;
    } else {
      binaryFrames++;
      frameBytes = event.data.byteLength;
    }
  };
  await until(() => binaryFrames === 1, "first unacknowledged frame");
  const firstSequence = ackSequence;
  await api({
    action: "input",
    tabId,
    lease: rawLease.lease,
    event: { type: "scroll", x: 100, y: 300, deltaX: 0, deltaY: 100 },
  });
  await new Promise((r) => setTimeout(r, 1200));
  assert.equal(binaryFrames, 1);
  raw.send(JSON.stringify({ type: "ack", sequence: firstSequence }));
  await until(() => binaryFrames === 2, "frame after acknowledgement");
  assert.ok(ackSequence > firstSequence);
  await api({ action: "release", tabId, lease: rawLease.lease, resume: true });
  raw.close();
  assert.ok(frameBytes < 1024 * 1024);
  passed("slow viewers receive one frame until ACK, then the latest frame");
  await js("window.scrollTo(0,0); return true;");
  await tool("chrome_request_handoff", { tabId, reason: "Confirm through a network that blocks browser streaming." });
  const fallbackContext = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  await fallbackContext.addInitScript(() => {
    class BlockedSocket { constructor() { throw Error("WebSocket upgrades are blocked on this network"); } }
    BlockedSocket.OPEN = 1;
    window.WebSocket = BlockedSocket;
  });
  const fallbackPage = await fallbackContext.newPage();
  let httpMeta, httpFrames = 0, takes = 0;
  fallbackPage.on("pageerror", error => errors.push(error.message));
  fallbackPage.on("response", async response => {
    if (!response.url().endsWith("/api/browser/command") || !response.ok()) return;
    const command = response.request().postDataJSON();
    const body = await response.json().catch(() => ({}));
    if (command.action === "take") { takes++; controllerLease = body.lease; }
    if (command.action === "frame" && body.frame) { httpMeta = body.frame.metadata; httpFrames++; }
  });
  await fallbackPage.goto(`${connection.url}/browser?sessionId=${broSessionId}#token=${connection.token}`);
  await fallbackPage.getByRole("button", { name: "Bro handoff fixture" }).click();
  await fallbackPage.locator("#frame:not([hidden])").waitFor({ timeout: 30_000 });
  await until(() => !!httpMeta, "HTTP fallback first frame");
  assert.equal(takes, 2, "release the failed socket's lease before taking a new one");
  const fallbackBox = await fallbackPage.locator("#frame").boundingBox();
  const fallbackScale = fallbackBox.width / httpMeta.deviceWidth;
  await fallbackPage.touchscreen.tap(fallbackBox.x + 120 * fallbackScale, fallbackBox.y + (183 + (httpMeta.offsetTop || 0)) * fallbackScale);
  await fallbackPage.locator("#keyboard").click();
  await fallbackPage.locator("#keyboard-input").fill(" + HTTP fallback café");
  await fallbackPage.locator("#done").click();
  await until(async () => (await tool("chrome_wait_for_handoff", { tabId, timeoutMs: 0 })).phase === "agent", "HTTP Done drains input and records completion");
  const fallbackState = await js("return {draft:draft.value,pageIdentity:instanceNonce,signedIn:(await (await fetch('/auth')).json()).signedIn};");
  assert.ok(fallbackState.draft.includes("HTTP fallback café"), fallbackState.draft);
  assert.equal(fallbackState.pageIdentity, before.pageIdentity);
  assert.equal(fallbackState.signedIn, true);
  assert.ok(httpFrames > 0);
  assert.deepEqual(errors, []);
  await fallbackPage.screenshot({ path: resolve(report, "http-fallback-done.png") });
  await fallbackContext.close();
  passed("blocked WebSocket upgrades recover over HTTP with signed-in state and ordered Done intact");
  await tool("chrome_request_handoff", { tabId, reason: "Check a frame finishing after the connection pauses." });
  const pausedContext = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await pausedContext.addInitScript(() => {
    const RealImage = window.Image;
    window.Image = class extends RealImage {
      decode() { return new Promise((resolve, reject) => { window.finishBrowserFrame = () => super.decode().then(resolve, reject); }); }
    };
  });
  const pausedPage = await pausedContext.newPage();
  await pausedPage.goto(`${connection.url}/browser?sessionId=${broSessionId}#token=${connection.token}`);
  await pausedPage.getByRole("button", { name: "Bro handoff fixture" }).click();
  await pausedPage.waitForFunction(() => !!window.finishBrowserFrame);
  await pausedPage.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await until(async () => (await tool("chrome_wait_for_handoff", { tabId, timeoutMs: 0 })).phase === "requested", "closing while decoding retains the handoff");
  await pausedPage.evaluate(() => window.finishBrowserFrame());
  assert.equal(await pausedPage.locator("#done").isDisabled(), true, "a late image must not reactivate disconnected controls");
  assert.equal(await pausedPage.locator("#status").textContent(), "Connection paused.");
  await pausedContext.close();
  passed("late image decoding cannot reactivate controls after the viewer disconnects");
  await writeFile(
    resolve(report, "results.json"),
    JSON.stringify(
      { engine, legacySession, passed: results, tabId, verifiedAt: new Date().toISOString() },
      null,
      2,
    ),
  );
  if (argument("--hold", "0") !== "0") {
    await tool("chrome_request_handoff", {
      tabId,
      reason: "Confirm the selection from Bro on Android.",
    });
    await writeFile(
      argument("--info") + ".fixture.json",
      JSON.stringify({ fixtureURL, tabId }),
    );
    console.log(
      "Native-device fixture ready; call its /finish endpoint to clean up.",
    );
    let holdTimer;
    await Promise.race([
      manualComplete,
      new Promise((r) => {
        holdTimer = setTimeout(r, Number(argument("--hold")) * 1000);
      }),
    ]);
    clearTimeout(holdTimer);
  }
} finally {
  if (tabId) {
    // Finish any outstanding human request before closing only our disposable fixture tab.
    if (controllerLease)
      await api({
        action: "release",
        tabId,
        lease: controllerLease,
        resume: true,
      }).catch(() => {});
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        const s = await api({ action: "take", tabId });
        await api({ action: "release", tabId, lease: s.lease, resume: true });
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 500));
      }
    }
    await js("await fetch('/cleanup'); return true;").catch(() => {});
    await tool("chrome_close_tabs", { tabIds: [tabId] }, true).catch(() => {});
  }
  if (mcpSession)
    await fetch("http://127.0.0.1:12306/mcp", {
      method: "DELETE",
      headers: { "mcp-session-id": mcpSession },
    }).catch(() => {});
  await browser?.close();
  await new Promise((r) => fixture.close(r));
}
