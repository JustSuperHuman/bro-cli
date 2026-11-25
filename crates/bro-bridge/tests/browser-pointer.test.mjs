import { test } from "node:test";
import assert from "node:assert/strict";
import { browserPoint, createBrowserPointer } from "../src/browser/pointer.mjs";

function fixture() {
  const sent = [], painted = [], views = [], timers = new Map();
  let time = 0, serial = 0;
  const geometry = {
    rect: { left: 20, top: 100, right: 420, bottom: 400, width: 400, height: 300 },
    meta: { deviceWidth: 1600, deviceHeight: 1200, offsetTop: 0 },
    stage: { left: 0, top: 0, width: 440, height: 700 },
  };
  let view = { zoom: 1, pan: { x: 0, y: 0 } };
  const pointer = createBrowserPointer({
    geometry: () => geometry, send: event => sent.push(event),
    onCursor: p => painted.push(p), getView: () => structuredClone(view),
    onView: v => { view = v; views.push(v); }, now: () => time,
    setTimer: (fn, delay) => { timers.set(++serial, { fn, at: time + delay }); return serial; },
    clearTimer: id => timers.delete(id),
  });
  function tick(ms) {
    time += ms;
    for (const [id, timer] of [...timers]) if (timer.at <= time) { timers.delete(id); timer.fn(); }
  }
  const event = (x, y, id = 1, pointerType = "touch", button = 0) => ({ clientX: x, clientY: y, pointerId: id, pointerType, button });
  pointer.sync();
  return { pointer, sent, painted, views, geometry, event, tick };
}

test("scaled frames map to remote CSS pixels and exclude letterboxing", () => {
  const rect = { left: 100, top: 50, right: 300, bottom: 450, width: 200, height: 400 };
  assert.deepEqual(browserPoint(rect, { deviceWidth: 400, offsetTop: 20 }, 200, 250), { x: 200, y: 380 });
  assert.equal(browserPoint(rect, { deviceWidth: 400 }, 50, 250), null);
  assert.equal(browserPoint(rect, { deviceWidth: 400, offsetTop: 20 }, 150, 55), null);
  assert.equal(browserPoint(rect, { deviceWidth: 0 }, 150, 100), null);
});

test("trackpad movement works in letterboxing and does not scroll or click", () => {
  const f = fixture();
  assert.equal(f.pointer.down(f.event(100, 600)), true);
  f.pointer.move(f.event(150, 620));
  assert.deepEqual(f.painted.at(-1), { x: 1000, y: 680 });
  assert.equal(f.sent.length, 0, "local cursor should paint before network input");
  f.pointer.up(f.event(150, 620));
  assert.deepEqual(f.sent, [{ type: "pointer", x: 1000, y: 680, action: "mouseMoved", buttons: 0 }]);
});

test("a tap clicks the existing cursor without jumping under the finger", () => {
  const f = fixture();
  f.pointer.down(f.event(60, 150)); f.pointer.up(f.event(60, 150));
  assert.deepEqual(f.sent, [{ type: "click", x: 800, y: 600 }]);
});

test("hover bursts coalesce but flush before a mouse press", () => {
  const f = fixture();
  for (let x = 40; x <= 140; x++) f.pointer.move(f.event(x, 200, 1, "mouse"));
  assert.equal(f.sent.length, 0);
  f.pointer.down(f.event(140, 200, 1, "mouse"));
  assert.deepEqual(f.sent.map(e => [e.action, e.x, e.buttons]), [["mouseMoved", 480, 0], ["mousePressed", 480, 1]]);
  f.tick(1000);
  assert.equal(f.sent.length, 2);
  f.pointer.cancel();
});

test("holding and dragging releases once when cancelled outside the frame", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.tick(350);
  f.pointer.move(f.event(140, 640));
  f.pointer.up(f.event(500, 900), true); f.pointer.cancel(); f.tick(1000);
  assert.deepEqual(f.sent.map(e => [e.action, e.buttons]), [["mousePressed", 1], ["mouseMoved", 1], ["mouseReleased", 0]]);
  assert.equal(f.sent.at(-1).x, 960);
  assert.equal(f.sent.some(e => e.type === "click"), false);
});

test("movement cancels the hold timer and lifting never clicks", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.pointer.move(f.event(120, 600)); f.tick(500);
  f.pointer.up(f.event(120, 600));
  assert.ok(f.sent.every(e => e.action === "mouseMoved" && e.buttons === 0));
});

test("two-finger scrolling leaves the cursor in place and cannot click on lift", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.pointer.down(f.event(200, 600, 2));
  f.pointer.move(f.event(100, 570)); f.pointer.move(f.event(200, 570, 2));
  f.pointer.up(f.event(100, 570)); f.pointer.move(f.event(200, 550, 2)); f.pointer.up(f.event(200, 550, 2)); f.tick(500);
  assert.ok(f.sent.length > 0 && f.sent.every(e => e.type === "scroll"));
  assert.equal(f.sent.reduce((sum, e) => sum + e.deltaY, 0), 120);
  assert.deepEqual(f.painted.at(-1), { x: 800, y: 600 });
});

test("pinching changes only the local view and remaining fingers cannot click", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 300)); f.pointer.down(f.event(200, 300, 2));
  f.pointer.move(f.event(50, 300)); f.pointer.move(f.event(250, 300, 2));
  f.pointer.up(f.event(50, 300)); f.pointer.up(f.event(250, 300, 2)); f.tick(500);
  assert.equal(f.views.at(-1).zoom, 2);
  assert.deepEqual(f.sent, []);
});

test("two-finger tapping right-clicks once and cancels a pending long press", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.pointer.down(f.event(200, 600, 2));
  f.pointer.up(f.event(100, 600)); f.pointer.up(f.event(200, 600, 2)); f.tick(500);
  assert.deepEqual(f.sent.map(e => [e.action, e.button]), [["mousePressed", "right"], ["mouseReleased", "right"]]);
});

test("direct touch remains available for clicking and one-finger scrolling", () => {
  const f = fixture(); f.pointer.setMode("touch");
  assert.equal(f.pointer.down(f.event(100, 600)), false);
  f.pointer.down(f.event(60, 150)); f.pointer.up(f.event(60, 150));
  assert.deepEqual(f.sent[0], { type: "click", x: 160, y: 200 });
  f.pointer.down(f.event(100, 250)); f.pointer.move(f.event(100, 220)); f.pointer.up(f.event(100, 220));
  assert.deepEqual(f.sent[1], { type: "scroll", x: 320, y: 480, deltaX: 0, deltaY: 120 });
});

test("cursor clamps at the remote bounds and keeps its coordinates across viewer resize", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.pointer.move(f.event(10000, -10000)); f.pointer.up(f.event(10000, -10000));
  assert.deepEqual(f.painted.at(-1), { x: 1599, y: 0 });
  f.geometry.rect = { left: 0, top: 20, right: 800, bottom: 620, width: 800, height: 600 };
  f.pointer.sync();
  assert.deepEqual(f.painted.at(-1), { x: 1599, y: 0 });
  f.pointer.down(f.event(100, 650)); f.pointer.move(f.event(90, 650)); f.pointer.up(f.event(90, 650));
  assert.deepEqual(f.painted.at(-1), { x: 1579, y: 0 });
});

test("changing modes releases a held drag and clears all pending gestures", () => {
  const f = fixture();
  f.pointer.down(f.event(100, 600)); f.tick(350); f.pointer.setMode("touch"); f.tick(1000);
  assert.deepEqual(f.sent.map(e => e.action), ["mousePressed", "mouseReleased"]);
  f.pointer.up(f.event(100, 600));
  assert.equal(f.sent.length, 2);
});

test("quick presses in one spot become double and triple clicks; a pause or a move starts over", () => {
  const f = fixture();
  const mouse = (x, y, extra = {}) => ({ ...f.event(x, y, 1, "mouse"), ...extra });
  for (let n = 0; n < 4; n++) { f.pointer.down(mouse(100, 200)); f.pointer.up(mouse(100, 200)); f.tick(120); }
  const counts = f.sent.filter(e => e.action === "mousePressed").map(e => e.clickCount || 1);
  assert.deepEqual(counts, [1, 2, 3, 3]);
  const releases = f.sent.filter(e => e.action === "mouseReleased").map(e => e.clickCount || 1);
  assert.deepEqual(releases, counts, "each release carries its press's count");
  f.tick(600); f.sent.length = 0;
  f.pointer.down(mouse(100, 200)); f.pointer.up(mouse(100, 200)); f.tick(50);
  f.pointer.down(mouse(160, 200)); f.pointer.up(mouse(160, 200));
  assert.deepEqual(f.sent.filter(e => e.action === "mousePressed").map(e => e.clickCount || 1), [1, 1]);
});

test("shift and ctrl ride along on clicks, drags and hover", () => {
  const f = fixture();
  const mouse = (x, y, extra = {}) => ({ ...f.event(x, y, 1, "mouse"), ...extra });
  f.pointer.down(mouse(100, 200, { shiftKey: true }));
  f.pointer.move(mouse(120, 210, { shiftKey: true, ctrlKey: true })); f.tick(40);
  f.pointer.up(mouse(120, 210, { shiftKey: true }));
  assert.deepEqual(f.sent.map(e => [e.action, e.modifiers, e.buttons]), [
    ["mousePressed", 8, 1], ["mouseMoved", 10, 1], ["mouseMoved", 8, 1], ["mouseReleased", 8, 0],
  ]);
});

test("a double tap in trackpad mode double-clicks under the cursor", () => {
  const f = fixture();
  f.pointer.down(f.event(60, 150)); f.pointer.up(f.event(60, 150)); f.tick(150);
  f.pointer.down(f.event(64, 152)); f.pointer.up(f.event(64, 152));
  assert.deepEqual(f.sent, [{ type: "click", x: 800, y: 600 }, { type: "click", x: 800, y: 600, clickCount: 2 }]);
});
