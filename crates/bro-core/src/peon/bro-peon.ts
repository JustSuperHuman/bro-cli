// bro's PeonPing hook for Pi and omp (oh-my-pi): their lifecycle events, as Claude Code-style hook events, handed
// to `bro hook peon <harness>` on stdin. bro loads this with `-e` into the sessions it starts (see
// crates/bro-core/src/peon/wire.rs); outside them (no BRO_PEON_BIN) it does nothing. Events that don't exist in
// one harness are simply never fired there.
import { spawn } from "node:child_process"

export default function broPeon(pi: any): void {
  const bin = process.env.BRO_PEON_BIN
  if (!bin) return
  const harness = process.env.BRO_PEON_HARNESS || "pi"
  const sessionId = `${harness}-${process.env.BRO_SESSION_ID || Date.now()}`
  // omp and pi can both announce one compaction; speak once
  const last: Record<string, number> = {}

  function fire(event: string, extra: Record<string, unknown> = {}): void {
    const now = Date.now()
    if (now - (last[event] ?? 0) < 1500) return
    last[event] = now
    try {
      const proc = spawn(bin as string, ["hook", "peon", harness], { stdio: ["pipe", "ignore", "ignore"], windowsHide: true })
      proc.on("error", () => {})
      proc.stdin?.on("error", () => {})
      proc.stdin?.end(JSON.stringify({ hook_event_name: event, session_id: sessionId, cwd: process.cwd(), ...extra }))
      proc.unref()
    } catch {}
  }

  // a subagent's own loop stays quiet (omp 18.3.2+ says which one it is)
  const sub = (ctx: any): boolean => ctx?.agent?.kind === "sub"
  function on(name: string, handler: (event: any) => void): void {
    try {
      pi.on(name, async (event: any, ctx: any) => {
        if (!sub(ctx)) handler(event ?? {})
      })
    } catch {}
  }

  on("session_start", () => fire("SessionStart"))
  on("agent_start", () => fire("UserPromptSubmit"))
  on("agent_end", (e) => {
    if (e.willContinue !== true) fire("Stop")
  })
  on("tool_result", (e) => {
    if (e.isError === true) fire("PostToolUseFailure", typeof e.toolName === "string" ? { tool_name: e.toolName } : {})
  })
  on("auto_compaction_start", () => fire("PreCompact"))
  on("session_before_compact", () => fire("PreCompact"))
  on("session_shutdown", () => fire("SessionEnd"))
}
