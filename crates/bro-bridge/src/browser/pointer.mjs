const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
/** CDP's modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8. */
export const modifierBits = (event) =>
  (event?.altKey ? 1 : 0) + (event?.ctrlKey ? 2 : 0) + (event?.metaKey ? 4 : 0) + (event?.shiftKey ? 8 : 0);
const BUTTON_BITS = { left: 1, right: 2, middle: 4 };
// Fields the gateway treats as defaults stay off the wire.
const extra = (modifiers, clickCount) => ({ ...(modifiers ? { modifiers } : {}), ...(clickCount > 1 ? { clickCount } : {}) });

export function browserGeometry(rect, meta) {
  if (!meta || !Number.isFinite(meta.deviceWidth) || meta.deviceWidth <= 0 || rect.width <= 0 || rect.height <= 0) return;
  const scale = rect.width / meta.deviceWidth;
  const offset = meta.offsetTop || 0;
  return { scale, offset, width: meta.deviceWidth, height: Math.max(1, rect.height / scale - offset) };
}

export function browserPoint(rect, meta, x, y) {
  const geometry = browserGeometry(rect, meta);
  if (!geometry || x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return null;
  const remoteY = (y - rect.top) / geometry.scale - geometry.offset;
  return remoteY < 0 ? null : { x: (x - rect.left) / geometry.scale, y: remoteY };
}

/** Keep the pointer in remote CSS pixels; the viewer's size and zoom only affect its projection. */
export function createBrowserPointer({ geometry, send, onCursor, getView, onView, now = Date.now, setTimer = setTimeout, clearTimer = clearTimeout }) {
  const touches = new Map();
  let cursor, gesture, multi, held, heldCount = 1, holdTimer, moveTimer, pendingMove, mode = "cursor";
  // Presses close together in time and place count up, so the page sees double and triple clicks.
  let lastPress = { at: -Infinity, x: 0, y: 0, button: "", count: 0 };
  function clickCount(button) {
    const t = now(), same = lastPress.button === button && t - lastPress.at < 500 && cursor &&
      Math.hypot(cursor.x - lastPress.x, cursor.y - lastPress.y) <= 6 / (metrics()?.scale || 1);
    lastPress = { at: t, x: cursor?.x || 0, y: cursor?.y || 0, button, count: same ? Math.min(3, lastPress.count + 1) : 1 };
    return lastPress.count;
  }
  const metrics = () => { const { rect, meta } = geometry(); return browserGeometry(rect, meta); };
  const project = event => { const { rect, meta } = geometry(); return browserPoint(rect, meta, event.clientX, event.clientY); };
  function place(point) {
    const bounds = metrics();
    if (!bounds) return;
    cursor = {
      x: clamp(point.x, 0, bounds.width - 1),
      y: clamp(point.y, 0, bounds.height - 1),
    };
    onCursor({ ...cursor });
    return cursor;
  }
  function sync() {
    const bounds = metrics();
    if (!bounds) return;
    if (!cursor) place({ x: bounds.width / 2, y: bounds.height / 2 });
    else if (cursor.x > bounds.width - 1 || cursor.y > bounds.height - 1) place(cursor);
  }
  function flush() {
    clearTimer(moveTimer); moveTimer = undefined;
    if (pendingMove) { send(pendingMove); pendingMove = undefined; }
  }
  function move(point, modifiers = 0) {
    const p = place(point);
    if (!p) return;
    pendingMove = { type: "pointer", ...p, action: "mouseMoved", buttons: BUTTON_BITS[held] || 0, ...extra(modifiers) };
    // Paint locally immediately. Coalesce remote hover to avoid filling the
    // gateway's ordered CDP queue with obsolete positions on a slow connection.
    if (!moveTimer) moveTimer = setTimer(flush, 32);
  }
  function press(button, modifiers = 0) {
    flush(); held = button; heldCount = clickCount(button);
    send({ type: "pointer", ...cursor, action: "mousePressed", button, buttons: BUTTON_BITS[button], ...extra(modifiers, heldCount) });
  }
  function release(modifiers = 0) {
    flush();
    if (held) send({ type: "pointer", ...cursor, action: "mouseReleased", button: held, buttons: 0, ...extra(modifiers, heldCount) });
    held = undefined;
  }
  function cancel() {
    clearTimer(holdTimer); holdTimer = undefined;
    release(); touches.clear(); gesture = multi = undefined;
  }
  function down(event) {
    if (!metrics()) return false;
    sync();
    const touch = event.pointerType !== "mouse";
    if ((!touch || mode === "touch") && !project(event)) return false;
    touches.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (touches.size >= 2) {
      clearTimer(holdTimer); release();
      if (gesture) gesture.moved = true;
      const [a, b] = [...touches.values()];
      multi = { distance: Math.hypot(a.x - b.x, a.y - b.y), x: (a.x + b.x) / 2, y: (a.y + b.y) / 2,
        lastX: (a.x + b.x) / 2, lastY: (a.y + b.y) / 2, view: getView(), time: now(), kind: touches.size > 2 ? "cancel" : undefined };
      return true;
    }
    gesture = { x: event.clientX, y: event.clientY, lastX: event.clientX, lastY: event.clientY, touch, moved: false };
    if (!touch) { place(project(event)); press(event.button === 2 ? "right" : "left", modifierBits(event)); }
    else if (mode === "touch") place(project(event));
    else holdTimer = setTimer(() => {
      if (gesture && !gesture.moved && touches.size === 1) { gesture.moved = true; press("left"); }
    }, 350);
    return true;
  }
  function moving(event) {
    if (!metrics()) return;
    if (!touches.has(event.pointerId)) {
      if (event.pointerType === "mouse") { const p = project(event); if (p) move(p, modifierBits(event)); }
      return;
    }
    touches.set(event.pointerId, { x: event.clientX, y: event.clientY });
    if (multi) {
      if (touches.size !== 2 || multi.kind === "cancel") return;
      const [a, b] = [...touches.values()];
      const distance = Math.hypot(a.x - b.x, a.y - b.y), x = (a.x + b.x) / 2, y = (a.y + b.y) / 2;
      if (!multi.kind) {
        if (Math.abs(distance - multi.distance) > Math.max(10, multi.distance * 0.08)) multi.kind = "pinch";
        else if (Math.hypot(x - multi.x, y - multi.y) > 6) multi.kind = "scroll";
      }
      if (multi.kind === "pinch") {
        const view = multi.view, zoom = clamp(view.zoom * distance / Math.max(1, multi.distance), 1, 4);
        const { stage } = geometry(), cx = stage.left + stage.width / 2, cy = stage.top + stage.height / 2;
        onView({ zoom, pan: {
          x: x - cx - (multi.x - cx - view.pan.x) * zoom / view.zoom,
          y: y - cy - (multi.y - cy - view.pan.y) * zoom / view.zoom,
        } });
      } else if (multi.kind === "scroll") {
        send({ type: "scroll", ...cursor, deltaX: (multi.lastX - x) / metrics().scale, deltaY: (multi.lastY - y) / metrics().scale });
      }
      if (multi.kind) { multi.lastX = x; multi.lastY = y; }
      return;
    }
    if (!gesture) return;
    if (!gesture.moved && Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) > 4) {
      gesture.moved = true; clearTimer(holdTimer);
    }
    const p = project(event);
    if (!gesture.touch) { if (p) move(p, modifierBits(event)); }
    else if (gesture.moved && mode === "cursor") {
      move({ x: cursor.x + (event.clientX - gesture.lastX) / metrics().scale, y: cursor.y + (event.clientY - gesture.lastY) / metrics().scale });
    } else if (gesture.moved && p) {
      place(p);
      send({ type: "scroll", ...p, deltaX: (gesture.lastX - event.clientX) / metrics().scale, deltaY: (gesture.lastY - event.clientY) / metrics().scale });
    }
    if (gesture.moved) { gesture.lastX = event.clientX; gesture.lastY = event.clientY; }
  }
  function up(event, cancelled = false) {
    if (!touches.has(event.pointerId)) return;
    clearTimer(holdTimer); holdTimer = undefined;
    if (!multi && gesture) {
      if (!gesture.touch) { const p = project(event); if (p) move(p, modifierBits(event)); release(modifierBits(event)); }
      else if (held) release();
      else if (!cancelled && !gesture.moved) {
        const p = mode === "cursor" ? cursor : project(event);
        if (p) { place(p); flush(); send({ type: "click", ...p, ...extra(0, clickCount("left")) }); }
      }
    } else if (multi && !cancelled && touches.size === 2 && !multi.kind && now() - multi.time < 350) {
      press("right"); release(); multi.kind = "cancel";
    }
    if (cancelled && multi) multi.kind = "cancel";
    touches.delete(event.pointerId);
    if (!touches.size) { gesture = multi = undefined; release(); }
  }
  return { down, move: moving, up, sync, flush, cancel,
    setMode(value) { cancel(); mode = value === "touch" ? "touch" : "cursor"; },
    reset() { cancel(); cursor = undefined; },
  };
}
