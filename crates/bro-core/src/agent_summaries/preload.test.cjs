const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");
const { test } = require("node:test");
const vm = require("node:vm");
const source = readFileSync(join(__dirname, "preload.cjs"), "utf8");

function runtime({ main = "/$bunfs/root/cli", executable = "/usr/bin/claude", enabled = "1", bun = true, log = false } = {}) {
  const calls = [];
  const lines = [];
  const exits = [];
  const timers = {
    setTimeout(callback, delay, ...args) {
      const handle = { unrefed: false, unref() { this.unrefed = true; } };
      calls.push({ callback, delay, args, handle, receiver: this });
      return handle;
    },
    setInterval() {},
  };
  const original = timers.setTimeout;
  original.__promisify__ = () => "legacy promise";
  original[Symbol.for("nodejs.util.promisify.custom")] = () => "custom promise";
  const context = vm.createContext({
    ...(bun ? { Bun: { main, version: "test" } } : {}),
    process: {
      env: { BRO_CLAUDE_SKIP_AGENT_SUMMARIES: enabled, ...(log ? { BRO_CLAUDE_AGENT_SUMMARY_LOG: "/debug.log" } : {}) },
      execPath: executable,
      pid: 123,
      on: (event, callback) => { assert.equal(event, "exit"); exits.push(callback); },
    },
    setTimeout: original,
    setInterval: timers.setInterval,
    require(name) {
      assert.equal(name, "fs");
      return { appendFileSync: (path, line) => { assert.equal(path, "/debug.log"); lines.push(line); } };
    },
  });
  vm.runInContext(source, context);
  return { context, calls, lines, exits, original, interval: timers.setInterval };
}

test("only Bro's opted-in Claude runtimes install the hook, including Windows", () => {
  for (const [main, executable] of [
    ["/$bunfs/root/cli", "/nix/store/claude/bin/.claude-wrapped"],
    ["/$bunfs/root/cli", "/home/me/.local/share/claude/versions/2.1.312"],
    ["/$bunfs/root/cli", "/node_modules/@anthropic-ai/claude-code/bin/claude.exe"],
    ["B:/~BUN/root/cli", "C:\\Users\\Someone Else\\.local\\bin\\claude.exe"],
    ["B:\\~BUN\\root\\cli", "C:\\Program Files\\Claude\\versions\\2.1.312.exe"],
  ]) {
    const { context, original } = runtime({ main, executable });
    assert.notEqual(context.setTimeout, original, `${main} ${executable}`);
  }
  for (const options of [
    { enabled: "0" }, { enabled: undefined }, { enabled: "" }, { bun: false },
    { main: "/work/build.ts", executable: "/usr/bin/bun" },
    { main: "/$bunfs/root/other", executable: "/usr/bin/claude" },
    { executable: "/usr/bin/some-other-app" },
    { executable: "C:\\work\\not-claude.exe" },
  ]) {
    // An absent env value must stay absent, rather than take runtime's default.
    const r = runtime({ ...options, enabled: Object.hasOwn(options, "enabled") ? String(options.enabled ?? "") : "1" });
    assert.equal(r.context.setTimeout, r.original, JSON.stringify(options));
  }
});

test("suppresses only anchored callbacks with an unrefed, clearable timer handle", () => {
  const { context, calls, interval } = runtime();
  let fired = false;
  function summary() { fired = true; return "[AgentSummary] Timer fired"; }
  const handle = context.setTimeout(summary, 30000, "argument");
  assert.equal(calls[0].delay, 2 ** 31 - 1);
  assert.equal(handle, calls[0].handle);
  assert.equal(handle.unrefed, true);
  calls[0].callback();
  assert.equal(fired, false);

  const callback = (value) => value;
  const receiver = {};
  context.setTimeout.call(receiver, callback, 25, "keep", 9);
  assert.equal(calls[1].callback, callback);
  assert.equal(calls[1].delay, 25);
  assert.deepEqual(calls[1].args, ["keep", 9]);
  assert.equal(calls[1].receiver, receiver);
  assert.equal(context.setInterval, interval);
  context.setTimeout(() => "agent_summary", 30000);
  assert.equal(calls[2].delay, 2 ** 31 - 1);
});

test("keeps promisify support and installs only once", () => {
  const { context, original } = runtime();
  const hooked = context.setTimeout;
  assert.equal(hooked.name, "setTimeout");
  assert.equal(hooked.__promisify__, original.__promisify__);
  const promisify = Symbol.for("nodejs.util.promisify.custom");
  assert.equal(hooked[promisify], original[promisify]);
  vm.runInContext(source, context);
  assert.equal(context.setTimeout, hooked);
});

test("unknown or bound callbacks pass through when source anchors disappear", () => {
  const { context, calls } = runtime();
  function futureCallback() { return "a renamed summary marker"; }
  function summary() { return "agent_summary"; }
  const bound = summary.bind(null);
  context.setTimeout(futureCallback, 30000);
  context.setTimeout(bound, 30000);
  assert.equal(calls[0].callback, futureCallback);
  assert.equal(calls[1].callback, bound);
  assert.equal(calls[0].delay, 30000);
  assert.equal(calls[1].delay, 30000);
});

test("diagnostics report installation and bounded suppression counts", () => {
  const { context, lines, exits } = runtime({ log: true });
  for (let i = 0; i < 25; i++) context.setTimeout(() => "agent_summary", 30000);
  exits[0]();
  assert.equal(lines.length, 22);
  assert.match(lines[0], /preload active .*hooked=global.setTimeout/);
  assert.match(lines[1], /summary timer #1 suppressed/);
  assert.match(lines.at(-1), /exit: 25 summary timer\(s\) suppressed/);
});
