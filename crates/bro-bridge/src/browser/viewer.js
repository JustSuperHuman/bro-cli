import { connectBrowser } from "./connection.js";
import { createBrowserPointer, browserGeometry, browserPoint, modifierBits } from "./pointer.js";
export { browserPoint };

const $ = (id) => document.getElementById(id);
const hash = new URLSearchParams(location.hash.slice(1));
const token =
  window.__BRO_BROWSER_TOKEN__ ||
  hash.get("token") ||
  sessionStorage.getItem("bro-browser-token") ||
  "";
if (token) sessionStorage.setItem("bro-browser-token", token);
if (hash.has("token"))
  history.replaceState(null, "", location.pathname + location.search);
const filterSession = new URLSearchParams(location.search).get("sessionId");
const targetTab = Number(new URLSearchParams(location.search).get("tabId"));
const targetHandoff = new URLSearchParams(location.search).get("handoffId");
if (new URLSearchParams(location.search).get("theme") === "dark")
  document.documentElement.dataset.theme = "dark";
let scopeOnly = !!filterSession;
let tabId,
  lease,
  socket,
  metadata,
  lastTab,
  sequence = 0,
  imageURL,
  generation = 0;
let zoom = 1,
  imageAspect,
  pan = { x: 0, y: 0 },
  composing = false,
  refreshTimer;
let returning = false,
  flushSequence = 0;
let manualHandoff;
let cursor;
const flushes = new Map();
const frame = $("frame"),
  stage = $("stage"),
  picker = $("picker");
// Safari's software keyboard resizes the visual viewport before layout catches
// up. Keep the controls above it in both the app's WKWebView and a browser tab.
function viewportHeight() {
  document.documentElement.style.setProperty(
    "--viewport-height",
    `${window.visualViewport?.height || innerHeight}px`,
  );
}
viewportHeight();
window.addEventListener("resize", viewportHeight);
window.visualViewport?.addEventListener("resize", viewportHeight);
$("keyboard").setAttribute("aria-label", "Show keyboard");
$("fit").setAttribute("aria-label", "Fit the remote page");
const controls = [
  "done",
  "back",
  "reload",
  "keyboard",
  "tab-key",
  "enter",
  "fit",
  "pointer-mode",
];
function state(text, enabled = false) {
  $("status").textContent = text;
  stage.dataset.live = String(enabled);
  if (!enabled) $("cursor").hidden = true;
  controls.forEach((id) => {
    $(id).disabled = !enabled;
  });
}
function notice(message) {
  $("notice").hidden = !message;
  $("notice").querySelector("span").textContent = message || "";
}
async function request(args) {
  const response = await fetch("api/browser" + (args ? "/command" : ""), {
    method: args ? "POST" : "GET",
    headers: {
      Authorization: `Bearer ${token}`,
      ...(args ? { "Content-Type": "application/json" } : {}),
    },
    ...(args ? { body: JSON.stringify(args) } : {}),
    signal: AbortSignal.timeout(35_000),
    cache: "no-store",
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok)
    throw new Error(
      body.message || `Browser connection failed (${response.status}).`,
    );
  return body;
}
function postNative(type) {
  window.ReactNativeWebView?.postMessage(JSON.stringify({ type }));
}
async function tabs() {
  clearTimeout(refreshTimer);
  const list = $("tab-list");
  try {
    const info = await request();
    const all = (info.tabs || []).filter(
      (tab) => !scopeOnly || tab.broSessionId === filterSession || (!tab.broSessionId && tab.phase === "requested"),
    );
    all.sort(
      (a, b) =>
        Number(b.phase === "requested") - Number(a.phase === "requested") ||
        Number(b.broSessionId === filterSession) -
          Number(a.broSessionId === filterSession),
    );
    list.replaceChildren();
    if (!all.length) {
      const p = document.createElement("p");
      p.textContent =
        "No web pages are open. Enter an address below to open one in your browser.";
      list.append(p);
    }
    for (const tab of all) {
      const button = document.createElement("button"),
        title = document.createElement("strong"),
        detail = document.createElement("span");
      title.textContent = tab.title || "Untitled tab";
      detail.textContent =
        tab.phase === "requested"
          ? tab.reason || "The agent needs your input."
          : tab.phase === "human" || tab.phase === "connecting"
            ? "In use on another screen"
            : tab.url;
      if (tab.phase === "requested") detail.className = "requested";
      button.append(title, detail);
      button.disabled = tab.phase === "human" || tab.phase === "connecting";
      button.addEventListener("click", () => openTab(tab));
      list.append(button);
    }
    if (scopeOnly) {
      const showAll = document.createElement("button");
      showAll.textContent = "Show all browser tabs";
      showAll.onclick = () => {
        scopeOnly = false;
        tabs();
      };
      list.append(showAll);
    }
  } catch (error) {
    list.replaceChildren();
    const p = document.createElement("p");
    p.textContent = error.message;
    list.append(p);
  }
  if (picker.open) refreshTimer = setTimeout(tabs, 4000);
}
function showPicker() {
  if (returning) return;
  if (!picker.open) picker.showModal();
  tabs();
}
async function openTab(tab) {
  if (returning) return;
  if (tab.viewerAvailable !== false) return take(tab.tabId);
  picker.close();
  clearTimeout(refreshTimer);
  await release();
  lastTab = undefined;
  manualHandoff = { tabId: tab.tabId, handoffId: tab.handoffId };
  notice("");
  $("title").textContent = tab.title || "Browser request";
  $("address").value = tab.url || "";
  $("empty").hidden = true;
  $("manual-handoff").hidden = false;
  $("manual-reason").textContent = tab.reason || "The agent needs your input.";
  state("Complete this step in your desktop browser.");
  $("done").textContent = "Return control";
  $("done").disabled = false;
  $("retry").textContent = "Refresh request";
}
function fit() {
  pointer.cancel();
  zoom = 1;
  pan = { x: 0, y: 0 };
  sizeFrame();
  transform();
  $("zoom").hidden = true;
}
function sizeFrame() {
  if (!imageAspect) return;
  const style = getComputedStyle(stage);
  const availableWidth =
    stage.clientWidth -
    parseFloat(style.paddingLeft) -
    parseFloat(style.paddingRight);
  const availableHeight =
    stage.clientHeight -
    parseFloat(style.paddingTop) -
    parseFloat(style.paddingBottom);
  const width = Math.max(
    1,
    Math.min(availableWidth, availableHeight * imageAspect),
  );
  frame.style.width = `${width}px`;
  frame.style.height = `${width / imageAspect}px`;
}
function transform() {
  const maxX = Math.max(0, (frame.offsetWidth * zoom - stage.clientWidth) / 2);
  const maxY = Math.max(0, (frame.offsetHeight * zoom - stage.clientHeight) / 2);
  pan.x = Math.max(-maxX, Math.min(maxX, pan.x));
  pan.y = Math.max(-maxY, Math.min(maxY, pan.y));
  frame.style.transform = `translate(${pan.x}px,${pan.y}px) scale(${zoom})`;
  paintCursor();
}
function paintCursor() {
  const overlay = $("cursor"), rect = frame.getBoundingClientRect(), box = stage.getBoundingClientRect();
  const geometry = browserGeometry(rect, metadata);
  overlay.hidden = !cursor || !geometry || frame.hidden || stage.dataset.live !== "true";
  if (overlay.hidden) return;
  overlay.style.transform = `translate(${rect.left - box.left + cursor.x * geometry.scale}px,${rect.top - box.top + (cursor.y + geometry.offset) * geometry.scale}px)`;
}
function followCursor() {
  if (!cursor || zoom === 1) { paintCursor(); return; }
  const rect = frame.getBoundingClientRect(), box = stage.getBoundingClientRect();
  const geometry = browserGeometry(rect, metadata);
  if (!geometry) return;
  const x = rect.left + cursor.x * geometry.scale, y = rect.top + (cursor.y + geometry.offset) * geometry.scale;
  const edge = 24;
  pan.x += Math.max(box.left + edge, Math.min(box.right - edge, x)) - x;
  pan.y += Math.max(box.top + edge, Math.min(box.bottom - edge, y)) - y;
  transform();
}
async function release(resume = false) {
  pointer.reset();
  flushScroll();
  if (resume && lease) await flushInputs();
  ++generation;
  const old = { tabId, lease };
  tabId = lease = undefined;
  metadata = undefined;
  cursor = undefined;
  $("cursor").hidden = true;
  const previousSocket = socket;
  socket = undefined;
  if (previousSocket) previousSocket.onclose = null;
  let error;
  if (old.lease) {
    try {
      await request({ action: "release", ...old, resume });
    } catch (caught) {
      error = caught;
    }
  }
  previousSocket?.close();
  frame.hidden = true;
  manualHandoff = undefined;
  $("manual-handoff").hidden = true;
  $("done").textContent = "Done";
  $("retry").textContent = "Reconnect";
  $("empty").hidden = false;
  $("connecting").hidden = true;
  $("keyboard-input").blur();
  $("keyboard-row").hidden = true;
  state(resume && !error ? "Ready for the agent." : "Your tab is still open.");
  if (resume && error) throw error;
}
function flushInputs() {
  pointer.flush();
  flushScroll();
  if (socket?.readyState !== WebSocket.OPEN)
    return Promise.reject(new Error("Reconnect before returning control."));
  const id = ++flushSequence;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      flushes.delete(id);
      reject(new Error("Browser input is still catching up. Try Done again."));
    }, 35_000);
    flushes.set(id, {
      resolve: () => {
        clearTimeout(timer);
        resolve();
      },
      reject: () => {
        clearTimeout(timer);
        reject(
          new Error(
            "The browser connection closed before your input finished.",
          ),
        );
      },
    });
    socket.send(JSON.stringify({ type: "flush", id }));
  });
}
async function take(id) {
  if (returning) return;
  picker.close();
  clearTimeout(refreshTimer);
  notice("");
  await release();
  lastTab = id;
  const own = ++generation;
  state("Connecting to your tab…");
  $("empty").hidden = true;
  $("connecting").hidden = false;
  fit();
  try {
    const takeRequest = {
      action: "take",
      tabId: id,
      mobile: false,
    };
    const opened = await request(takeRequest);
    if (own !== generation) {
      await request({
        action: "release",
        tabId: id,
        lease: opened.lease,
      }).catch(() => {});
      return;
    }
    tabId = id;
    lease = opened.lease;
    sequence = 0;
    const url = new URL("api/browser/stream", location.href);
    url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
    url.searchParams.set("token", token);
    const ws = connectBrowser({
      url, session: opened, take: takeRequest, request,
      onLease: (next) => { if (own === generation) lease = next.lease; },
    });
    socket = ws;
    ws.binaryType = "arraybuffer";
    let pendingFrame, lastError, connected = false;
    ws.onmessage = async (event) => {
      if (own !== generation) return;
      if (typeof event.data === "string") {
        const value = JSON.parse(event.data);
        if (value.type === "error") {
          lastError = value.message;
          notice(lastError);
          return;
        }
        if (value.type === "flushed") {
          flushes.get(value.id)?.resolve();
          flushes.delete(value.id);
          return;
        }
        if (value.type === "frame") {
          pendingFrame = value;
        }
        return;
      }
      const next = URL.createObjectURL(
        new Blob([event.data], { type: "image/jpeg" }),
      );
      const info = pendingFrame;
      pendingFrame = undefined;
      try {
        const decoded = new Image();
        decoded.src = next;
        await decoded.decode();
        if (own !== generation || ws.readyState !== WebSocket.OPEN) {
          URL.revokeObjectURL(next);
          return;
        }
        const previous = imageURL;
        if (!connected) { notice(""); connected = true; }
        frame.src = next;
        imageURL = next;
        imageAspect = decoded.naturalWidth / decoded.naturalHeight;
        sizeFrame();
        frame.hidden = false;
        metadata = info.frame.metadata;
        sequence = info.frame.sequence;
        $("title").textContent = info.title || "Bro Browser";
        if (document.activeElement !== $("address"))
          $("address").value = info.url || "";
        if (previous) URL.revokeObjectURL(previous);
        $("connecting").hidden = true;
        if (!returning) state("Live · your signed-in tab", true);
        transform();
        pointer.sync();
        if (ws.readyState === WebSocket.OPEN)
          ws.send(JSON.stringify({ type: "ack", sequence }));
      } catch {
        URL.revokeObjectURL(next);
        lastError = "This frame could not load. Reconnect to continue.";
        notice(lastError);
        ws.close();
      }
    };
    ws.onerror = () => {
      lastError =
        "The browser connection was interrupted. Reconnect to continue.";
      notice(lastError);
    };
    ws.onclose = () => {
      if (own === generation) {
        for (const pending of flushes.values()) pending.reject();
        flushes.clear();
        state("Connection paused.");
        pointer.cancel();
        $("connecting").hidden = true;
        notice(
          lastError ||
            "The connection closed. Your tab is still open in your browser.",
        );
      }
    };
  } catch (error) {
    if (own === generation) {
      $("connecting").hidden = true;
      $("empty").hidden = false;
      notice(error.message);
      state("Browser unavailable.");
    }
  }
}
let catchingUp;
function input(event) {
  if (returning || socket?.readyState !== WebSocket.OPEN || !metadata) return;
  // Keep pointer bursts bounded on slow mobile links.
  if (socket.bufferedAmount > 64 * 1024) {
    notice("The connection is catching up. Wait a moment.");
    clearTimeout(catchingUp);
    catchingUp = setTimeout(() => { if ($("notice").querySelector("span").textContent.startsWith("The connection is catching up")) notice(""); }, 1500);
    return;
  }
  socket.send(JSON.stringify({ type: "input", event }));
}
new ResizeObserver(() => {
  pointer.cancel();
  sizeFrame();
  transform();
  pointer.sync();
  followCursor();
}).observe(stage);
const point = (event) =>
  browserPoint(
    frame.getBoundingClientRect(),
    metadata,
    event.clientX,
    event.clientY,
  );
const pointer = createBrowserPointer({
  geometry: () => ({ rect: frame.getBoundingClientRect(), meta: metadata, stage: stage.getBoundingClientRect() }),
  send: event => event.type === "scroll" ? queueScroll(event, event.deltaX, event.deltaY) : input(event),
  onCursor: position => { cursor = position; followCursor(); },
  getView: () => ({ zoom, pan: { ...pan } }),
  onView: view => { zoom = view.zoom; pan = view.pan; transform(); zoomChip(); },
});
// Zoomed in, the middle button drags the view, and the mouse resting near an edge glides it that way.
let panDrag, edgeFrame, lastMouse;
const live = () => lease && !frame.hidden && !returning && socket?.readyState === WebSocket.OPEN;
function endPan() {
  panDrag = undefined;
  delete stage.dataset.panning;
}
function edgePush(value, low, high, zone) {
  if (value < low + zone) return Math.min(1.4, (low + zone - value) / zone);
  if (value > high - zone) return -Math.min(1.4, (value - (high - zone)) / zone);
  return 0;
}
function edgePan() {
  edgeFrame = undefined;
  if (!lastMouse || zoom <= 1 || panDrag || !live()) return;
  const box = stage.getBoundingClientRect(), zone = Math.min(64, box.width / 6, box.height / 6);
  const dx = edgePush(lastMouse.clientX, box.left, box.right, zone), dy = edgePush(lastMouse.clientY, box.top, box.bottom, zone);
  if (!dx && !dy) return;
  const before = { ...pan }, speed = 16;
  pan.x += Math.sign(dx) * dx * dx * speed;
  pan.y += Math.sign(dy) * dy * dy * speed;
  transform();
  if (pan.x === before.x && pan.y === before.y) return;
  // the page slid under a still mouse: what it points at changed
  pointer.move(lastMouse);
  edgeFrame = requestAnimationFrame(edgePan);
}
function watchEdges(event) {
  if (event.pointerType !== "mouse") return;
  lastMouse = { clientX: event.clientX, clientY: event.clientY, pointerId: event.pointerId, pointerType: "mouse",
    altKey: event.altKey, ctrlKey: event.ctrlKey, metaKey: event.metaKey, shiftKey: event.shiftKey };
  if (!edgeFrame && zoom > 1) edgeFrame = requestAnimationFrame(edgePan);
}
stage.addEventListener("mousedown", (event) => { if (event.button === 1) event.preventDefault(); });
stage.addEventListener("pointerdown", (event) => {
  if (event.pointerType === "mouse" && event.button === 1) {
    event.preventDefault();
    if (!live()) return;
    stage.focus({ preventScroll: true });
    if (zoom > 1) {
      panDrag = { id: event.pointerId, x: event.clientX, y: event.clientY, pan: { ...pan } };
      stage.dataset.panning = "true";
      stage.setPointerCapture(event.pointerId);
    }
    return;
  }
  if (!live() || !pointer.down(event)) return;
  stage.focus({ preventScroll: true });
  stage.setPointerCapture(event.pointerId);
  event.preventDefault();
});
stage.addEventListener("pointermove", (event) => {
  if (panDrag && event.pointerId === panDrag.id) {
    pan = { x: panDrag.pan.x + event.clientX - panDrag.x, y: panDrag.pan.y + event.clientY - panDrag.y };
    transform();
    return;
  }
  if (!live()) return;
  pointer.move(event);
  watchEdges(event);
});
stage.addEventListener("pointerleave", (event) => {
  if (event.pointerType === "mouse" && !stage.hasPointerCapture?.(event.pointerId)) lastMouse = undefined;
});
stage.addEventListener("pointerup", (event) => {
  if (panDrag && event.pointerId === panDrag.id) { endPan(); return; }
  pointer.up(event);
});
stage.addEventListener("pointercancel", (event) => {
  if (panDrag && event.pointerId === panDrag.id) { endPan(); return; }
  pointer.up(event, true);
});
stage.addEventListener("lostpointercapture", (event) => {
  if (panDrag && event.pointerId === panDrag.id) { endPan(); return; }
  pointer.cancel();
});
stage.addEventListener("contextmenu", (event) => event.preventDefault());
$("pointer-mode").onclick = () => {
  const enabled = $("pointer-mode").getAttribute("aria-pressed") !== "true";
  pointer.setMode(enabled ? "cursor" : "touch");
  $("pointer-mode").setAttribute("aria-pressed", String(enabled));
  $("pointer-mode").setAttribute("aria-label", enabled ? "Cursor mode. Switch to direct touch" : "Direct touch. Switch to cursor mode");
  $("pointer-mode").querySelector("span").textContent = enabled ? "Cursor" : "Touch";
  $("gesture-hint").textContent = enabled ? "Slide to move · Tap to click · Two fingers scroll" : "Tap to click · Slide to scroll · Pinch to zoom";
  stage.setAttribute("aria-label", enabled ? "Remote browser trackpad. Slide to move the cursor, tap to click, hold to drag, two fingers to scroll or pinch to zoom." : "Remote browser. Tap to click, slide to scroll, or pinch to zoom.");
};
let wheel, wheelTimer;
function flushScroll() {
  clearTimeout(wheelTimer); wheelTimer = undefined;
  if (wheel) { input({ type: "scroll", ...wheel }); wheel = undefined; }
}
function queueScroll(p, dx, dy) {
  wheel = {
    ...p,
    deltaX: Math.max(-2500, Math.min(2500, (wheel?.deltaX || 0) + dx)),
    deltaY: Math.max(-2500, Math.min(2500, (wheel?.deltaY || 0) + dy)),
  };
  if (!wheelTimer)
    wheelTimer = setTimeout(flushScroll, 35);
}
// Zoom the view (never the remote page) toward a point in the stage; 1x is the fitted page.
function zoomView(next, x, y) {
  next = Math.max(1, Math.min(4, next));
  if (next === zoom) return;
  pointer.cancel();
  const box = stage.getBoundingClientRect(),
    cx = box.left + box.width / 2,
    cy = box.top + box.height / 2;
  pan = {
    x: x - cx - ((x - cx - pan.x) * next) / zoom,
    y: y - cy - ((y - cy - pan.y) * next) / zoom,
  };
  zoom = next;
  if (zoom === 1) pan = { x: 0, y: 0 };
  transform();
  zoomChip();
  if (zoom > 1 && lastMouse && !edgeFrame) edgeFrame = requestAnimationFrame(edgePan);
}
function zoomChip() {
  const chip = $("zoom");
  chip.hidden = Math.abs(zoom - 1) < 0.005;
  chip.textContent = `${Math.round(zoom * 100)}%`;
  chip.title = "Reset zoom";
}
$("zoom").onclick = () => { fit(); zoomChip(); stage.focus({ preventScroll: true }); };
stage.addEventListener(
  "wheel",
  (event) => {
    if (event.ctrlKey) {
      event.preventDefault();
      zoomView(zoom * Math.exp(-event.deltaY * 0.0025), event.clientX, event.clientY);
      return;
    }
    const p = point(event);
    if (p) {
      event.preventDefault();
      const modifiers = modifierBits(event);
      queueScroll(modifiers ? { ...p, modifiers } : p, event.deltaX, event.deltaY);
    }
  },
  { passive: false },
);
const key = (value, modifiers = 0, autoRepeat = false) =>
  input({ type: "key", key: value, modifiers, ...(autoRepeat ? { autoRepeat } : {}) });
const NAMED_KEYS = new Set([
  "Enter", "Tab", "Backspace", "Delete", "Escape", "Insert",
  "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown",
  ...Array.from({ length: 12 }, (_, n) => "F" + (n + 1)),
]);
stage.addEventListener("keydown", (event) => {
  // On a web page Shift+Tab leaves the stage; inside Bro's window the page gets it.
  if (event.key === "Tab" && event.shiftKey && !window.ipc) return;
  if ((event.ctrlKey || event.metaKey) && !event.altKey && ["+", "=", "-", "_", "0"].includes(event.key)) {
    event.preventDefault();
    const box = stage.getBoundingClientRect(), cx = box.left + box.width / 2, cy = box.top + box.height / 2;
    if (event.key === "0") { fit(); zoomChip(); }
    else zoomView(zoom * (event.key === "-" || event.key === "_" ? 0.8 : 1.25), cx, cy);
    return;
  }
  const modifiers = modifierBits(event);
  if (NAMED_KEYS.has(event.key)) {
    event.preventDefault();
    key(event.key, modifiers, event.repeat);
  } else if ([...event.key].length !== 1) {
    // Shift, Control and Alt alone ride along on the keys and clicks they modify
    return;
  } else if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "v") {
    return;
  } else if ((event.ctrlKey && event.altKey) || (!event.ctrlKey && !event.metaKey)) {
    // a character (AltGr arrives as Ctrl+Alt on Windows): type it
    event.preventDefault();
    input({ type: "text", text: event.key });
  } else {
    event.preventDefault();
    key(event.key, modifiers, event.repeat);
  }
});
stage.addEventListener("paste", (event) => {
  const text = event.clipboardData?.getData("text/plain");
  if (text) {
    event.preventDefault();
    input({ type: "text", text });
  }
});
const keyboard = $("keyboard-input");
keyboard.addEventListener("compositionstart", () => {
  composing = true;
});
keyboard.addEventListener("compositionend", (event) => {
  composing = false;
  if (event.data) input({ type: "text", text: event.data });
  keyboard.value = "";
});
keyboard.addEventListener("input", (event) => {
  if (composing || event.isComposing) return;
  if (keyboard.value) input({ type: "text", text: keyboard.value });
  keyboard.value = "";
});
keyboard.addEventListener("beforeinput", (event) => {
  if (event.inputType === "deleteContentBackward") {
    event.preventDefault();
    key("Backspace");
  } else if (event.inputType === "insertLineBreak") {
    event.preventDefault();
    key("Enter");
  }
});
keyboard.addEventListener("keydown", (event) => {
  if (["Enter", "Tab", "Escape"].includes(event.key)) {
    event.preventDefault();
    key(event.key);
  }
});
$("keyboard").onclick = () => {
  $("keyboard-row").hidden = false;
  keyboard.focus();
};
$("hide-keyboard").onclick = () => {
  keyboard.blur();
  $("keyboard-row").hidden = true;
};
$("tab-key").onclick = () => key("Tab");
$("enter").onclick = () => key("Enter");
$("fit").onclick = fit;
$("back").onclick = () => input({ type: "back" });
$("reload").onclick = () => input({ type: "reload" });
$("done").onclick = async () => {
  if (returning) return;
  pointer.cancel();
  flushScroll();
  returning = true;
  state("Returning control…");
  try {
    if (manualHandoff) {
      await request({ action: "resume", ...manualHandoff });
      await release();
      state("Ready for the agent.");
    } else {
      await release(true);
    }
    notice("");
    postNative("done");
  } catch (error) {
    notice(
      error.message ||
        "Done could not reach Bro. Reconnect, then tap Done again.",
    );
    if (manualHandoff) {
      state("Control has not returned. Try again when the request is complete.");
      $("done").disabled = false;
    } else {
      state(
        socket?.readyState === WebSocket.OPEN
          ? "Live · your signed-in tab"
          : "Connection paused.",
        socket?.readyState === WebSocket.OPEN,
      );
    }
  } finally {
    returning = false;
  }
};
// Inside Bro's own window the page fills the window: Back leaves it without handing control over.
if (window.ipc) {
  document.documentElement.dataset.embedded = "true";
  $("leave").hidden = false;
  $("leave").onclick = async () => {
    if (returning) return;
    pointer.cancel();
    flushScroll();
    await release(false).catch(() => {});
    window.ipc.postMessage("close");
  };
}
$("tabs").onclick = $("choose").onclick = showPicker;
$("close-picker").onclick = () => {
  picker.close();
  clearTimeout(refreshTimer);
};
$("retry").onclick = () => (lastTab ? take(lastTab) : showPicker());
export function browserAddress(value) {
  let url = value.trim();
  if (!url) return;
  if (!/^https?:\/\//i.test(url))
    url =
      /\s/.test(url) || !url.includes(".")
        ? "https://www.google.com/search?q=" + encodeURIComponent(url)
        : "https://" + url;
  return url;
}
$("address-form").onsubmit = (event) => {
  event.preventDefault();
  const url = browserAddress($("address").value);
  if (url) input({ type: "navigate", url });
  $("address").blur();
};
$("new-form").onsubmit = async (event) => {
  event.preventDefault();
  try {
    const opened = await request({
      action: "open",
      url: browserAddress($("new-address").value),
    });
    await take(opened.tabId);
  } catch (error) {
    notice(error.message);
  }
};
$("desktop").onclick = () => {
  if (window.ReactNativeWebView) postNative("desktop");
  else
    notice(
      "Open Remote control in Bro to interact with file pickers or browser dialogs.",
    );
};
window.addEventListener("pagehide", () => {
  pointer.cancel();
  clearTimeout(wheelTimer); wheel = undefined;
  socket?.close();
});
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    pointer.cancel();
    clearTimeout(wheelTimer); wheel = undefined;
    socket?.close();
  } else if (lease) {
    state("Connection paused.");
    notice("Reconnect to continue using your tab.");
  }
});
if (!token) {
  notice("Open this page from a paired Bro app or Bro’s Browser command.");
  state("Pair with Bro to continue.");
} else if (Number.isInteger(targetTab) && targetTab > 0 && targetHandoff) {
  request().then(info => {
    const target = info.tabs?.find(tab => tab.tabId === targetTab && tab.handoffId === targetHandoff && tab.phase === "requested");
    if (target) return openTab(target);
    showPicker();
    notice("This handoff changed. Choose a tab to continue.");
  }).catch(error => { showPicker(); notice(error.message); });
} else showPicker();
