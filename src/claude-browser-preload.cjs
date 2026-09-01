// Claude Code's Windows browser bridge uses one named pipe per OS user. That
// makes every Edge profile compete for the same bridge even when Claude Code's
// login is isolated with CLAUDE_CONFIG_DIR. bro gives each login a stable pipe
// namespace by rewriting only that one pipe path at the Node/Bun net boundary.

const net = require('node:net');
// Bun exposes `net` and `node:net` as distinct module namespace objects even
// though their Socket/Server classes are shared. Claude's bundle imports the
// bare form, so patch both exported connect functions.
const bareNet = require('net');
const fs = require('node:fs');

const id = String(process.env.BRO_CLAUDE_BROWSER_ID || '').replace(/[^a-z0-9-]/gi, '');
const marker = '\\\\.\\pipe\\claude-mcp-browser-bridge-';
const patched = Symbol.for('bro.claudeBrowserPipePatched');
const traceFile = process.env.BRO_CLAUDE_BROWSER_TRACE_FILE;

if (traceFile) {
  try {
    fs.appendFileSync(traceFile, `${process.pid} preload id=${id || '(none)'}\n`);
  } catch {
    /* diagnostics must never interfere with Claude */
  }
}

function rewrite(value) {
  if (
    typeof value === 'string'
    && id
    && value.startsWith(marker)
    && !value.startsWith(`${marker}bro-${id}-`)
  ) {
    return `${marker}bro-${id}-${value.slice(marker.length)}`;
  }
  if (value && typeof value === 'object' && typeof value.path === 'string') {
    return { ...value, path: rewrite(value.path) };
  }
  return value;
}

function patch(target, name) {
  const original = target[name];
  if (typeof original !== 'function') return;
  target[name] = function (...args) {
    if (args.length) {
      const before = args[0];
      args[0] = rewrite(before);
      if (traceFile && before !== args[0]) {
        const from = typeof before === 'string' ? before : before?.path;
        const to = typeof args[0] === 'string' ? args[0] : args[0]?.path;
        try {
          fs.appendFileSync(traceFile, `${process.pid} ${name} ${from} -> ${to}\n`);
        } catch {
          /* diagnostics must never interfere with Claude */
        }
      }
    }
    return original.apply(this, args);
  };
}

if (id && process.platform === 'win32' && !globalThis[patched]) {
  globalThis[patched] = id;
  for (const target of new Set([net.Server.prototype, bareNet.Server.prototype])) patch(target, 'listen');
  for (const target of new Set([net.Socket.prototype, bareNet.Socket.prototype])) patch(target, 'connect');
  // Bun's net.connect/createConnection can bypass Socket.prototype.connect,
  // while Node normally delegates to it. Cover both runtimes; rewrite() is
  // idempotent, so a delegated call remains safe.
  for (const module of new Set([net, bareNet])) {
    patch(module, 'connect');
    patch(module, 'createConnection');
  }
}
