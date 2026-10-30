// Adapted from no-agent-summary.js v3 by Flamerix AI (MIT).
// Copyright (c) 2026 Flamerix AI (https://flamerix.ai).
// Contributions: Bedz01, Claude-only gating and --preload=.
// Source and full license: NOTICE. Bro adds Windows paths and a per-launch opt-in.
(() => {
  const installed = Symbol.for("bro.claude-agent-summaries.installed");
  if (process.env.BRO_CLAUDE_SKIP_AGENT_SUMMARIES !== "1" || globalThis[installed]) return;
  if (typeof Bun === "undefined") return;
  const main = String(Bun.main).replaceAll("\\", "/");
  if (main !== "/$bunfs/root/cli" && !/^[a-z]:\/~BUN\/root\/cli$/i.test(main)) return;
  const executable = process.execPath.replaceAll("\\", "/");
  const base = executable.slice(executable.lastIndexOf("/") + 1);
  if (!["claude", "claude.exe", ".claude-wrapped"].includes(base.toLowerCase()) &&
      !/\/claude\/versions\/\d+\.\d+\.\d+(?:\.exe)?$/i.test(executable)) return;

  const original = globalThis.setTimeout;
  if (typeof original !== "function") return;
  const anchors = ["[AgentSummary] Timer fired", "agent_summary"];
  const verdicts = new WeakMap();
  const logPath = process.env.BRO_CLAUDE_AGENT_SUMMARY_LOG;
  const log = (message) => {
    if (!logPath) return;
    try {
      require("fs").appendFileSync(logPath, `${new Date().toISOString()} pid=${process.pid} ${message}\n`);
    } catch {}
  };
  let suppressed = 0;
  function isSummaryTimer(callback) {
    if (typeof callback !== "function") return false;
    if (!verdicts.has(callback)) {
      let matches = false;
      try {
        const source = Function.prototype.toString.call(callback);
        matches = anchors.some((anchor) => source.includes(anchor));
      } catch {}
      verdicts.set(callback, matches);
    }
    return verdicts.get(callback);
  }
  function setTimeout(callback, delay, ...args) {
    if (!isSummaryTimer(callback)) return Reflect.apply(original, this, [callback, delay, ...args]);
    suppressed++;
    if (suppressed <= 20) log(`summary timer #${suppressed} suppressed (setTimeout, delay=${delay})`);
    // Larger delays overflow to 1 ms in Bun/Node. This handle remains clearTimeout-compatible
    // and does not keep the process alive. It never invokes Claude's summary callback.
    const timer = Reflect.apply(original, this, [() => {}, 2 ** 31 - 1]);
    if (timer && typeof timer.unref === "function") timer.unref();
    return timer;
  }
  const promisify = Symbol.for("nodejs.util.promisify.custom");
  if (original.__promisify__) setTimeout.__promisify__ = original.__promisify__;
  if (original[promisify]) setTimeout[promisify] = original[promisify];
  globalThis.setTimeout = setTimeout;
  globalThis[installed] = true;
  log(`preload active bun=${Bun.version} hooked=global.setTimeout`);
  if (logPath) process.on("exit", () => log(`exit: ${suppressed} summary timer(s) suppressed`));
})();
