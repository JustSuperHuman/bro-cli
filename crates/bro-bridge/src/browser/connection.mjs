/** One bounded stream, with HTTP recovery for networks that reject WebSocket upgrades. */
export function connectBrowser({ url, session, take, request, onLease, timeoutMs = 5000, socketFactory = url => new WebSocket(url) }) {
  let wire, stopped = false, fallback = false, received = false, timer;
  let pending = 0, tail = Promise.resolve(), expectedAck, acknowledge;
  let credentials = { tabId: session.tabId, lease: session.lease };
  const connection = {
    readyState: 0,
    get bufferedAmount() { return fallback ? pending * 2048 : wire?.bufferedAmount || 0; },
    send(text) {
      if (stopped || connection.readyState !== 1) return;
      if (!fallback) { wire.send(text); return; }
      const message = JSON.parse(text);
      if (message.type === "ack") {
        if (message.sequence === expectedAck) acknowledge?.();
        return;
      }
      if (!["input", "flush"].includes(message.type)) return;
      if (++pending > 32) { fail("The connection is catching up. Reconnect to continue."); return; }
      tail = tail.then(async () => {
        if (stopped) return;
        if (message.type === "input") {
          await request({ action: "input", ...credentials, event: message.event });
        } else emit({ type: "flushed", id: message.id });
      }).catch(error => fail(error.message)).finally(() => { --pending; });
    },
    close() {
      if (stopped) return;
      stopped = true;
      connection.readyState = 3;
      clearTimeout(timer);
      acknowledge?.();
      if (wire) { wire.onclose = wire.onerror = wire.onmessage = wire.onopen = null; wire.close(); }
      // Closing a viewer never returns control to the agent.
      void request({ action: "release", ...credentials }).catch(() => {});
      connection.onclose?.();
    },
  };
  function emit(value) { if (!stopped) connection.onmessage?.({ data: JSON.stringify(value) }); }
  function fail(message) {
    if (stopped) return;
    emit({ type: "error", message: message || "The browser connection ended. Reconnect to continue." });
    connection.close();
  }
  async function poll() {
    let after = 0;
    while (!stopped) {
      const info = await request({ action: "frame", ...credentials, after });
      if (stopped) return;
      if (info.frame) {
        const { data, ...frame } = info.frame;
        if (typeof data !== "string" || data.length > 4 * 1024 * 1024)
          throw Error("This browser frame could not load. Reconnect to continue.");
        const binary = Uint8Array.from(atob(data), character => character.charCodeAt(0));
        expectedAck = frame.sequence;
        const ack = new Promise(resolve => { acknowledge = resolve; });
        timer = setTimeout(() => fail("This browser frame could not load. Reconnect to continue."), 15_000);
        emit({ ...info, type: "frame", frame });
        if (!stopped) connection.onmessage?.({ data: binary.buffer });
        await ack;
        clearTimeout(timer);
        acknowledge = undefined;
        after = frame.sequence;
      }
      // Poll only after the previous image has been decoded and acknowledged.
      if (!stopped) await new Promise(resolve => { timer = setTimeout(resolve, 80); acknowledge = resolve; });
      acknowledge = undefined;
    }
  }
  async function recover() {
    if (stopped || fallback) return;
    fallback = true;
    connection.readyState = 0;
    clearTimeout(timer);
    if (wire) { wire.onclose = wire.onerror = wire.onmessage = wire.onopen = null; wire.close(); }
    try {
      // The failed socket may already be releasing this lease. Wait for the same
      // cleanup before claiming a new one, so it cannot detach the new stream.
      await request({ action: "release", ...credentials });
      if (stopped) return;
      const next = await request(take);
      credentials = { tabId: next.tabId, lease: next.lease };
      if (stopped) { await request({ action: "release", ...credentials }); return; }
      onLease(next);
      connection.readyState = 1;
      connection.onopen?.();
      await poll();
    } catch (error) { fail(error.message); }
  }
  queueMicrotask(() => {
    if (stopped) return;
    timer = setTimeout(recover, timeoutMs);
    try {
      wire = socketFactory(url);
      wire.binaryType = "arraybuffer";
      wire.onopen = () => {
        if (stopped || fallback) return;
        wire.send(JSON.stringify(credentials));
        connection.readyState = 1;
        connection.onopen?.();
      };
      wire.onmessage = event => {
        if (stopped || fallback) return;
        if (typeof event.data !== "string") { received = true; clearTimeout(timer); }
        connection.onmessage?.(event);
      };
      wire.onerror = () => { if (!received) void recover(); };
      wire.onclose = () => {
        if (!received) void recover();
        else connection.close();
      };
    } catch { void recover(); }
  });
  return connection;
}
