/**
 * Spawns the Claude Code CLI as a subprocess against a specific account's
 * config dir, parses its newline-delimited JSON, and re-exposes it as a single
 * normalized async stream of turn events that the API adapters consume.
 *
 * Using Bun.spawn (not child_process) and passing the prompt as stdin avoids
 * argv length limits and any shell interpretation.
 */

import type { CliMessage, CliResult, CliUsage } from "./types.ts";
import { isAssistant, isResult, isStreamEvent } from "./types.ts";

export type ClaudeModelAlias = "opus" | "sonnet" | "haiku";

export interface RunOptions {
  claudeBin: string;
  /** The account's CLAUDE_CONFIG_DIR. */
  configDir: string;
  model: ClaudeModelAlias;
  /** Optional fixed session id (UUID) passed to the CLI. */
  sessionId?: string;
  /** Extra text appended to the CLI's system prompt. */
  appendSystemPrompt?: string;
  timeoutMs: number;
  /** Aborts the subprocess (e.g. when the HTTP client disconnects). */
  signal?: AbortSignal;
}

/** Normalized events emitted regardless of which wire protocol we serve. */
export type TurnEvent =
  | { kind: "text"; text: string }
  | { kind: "text_block_boundary" }
  | { kind: "tool_use"; id: string; name: string }
  | { kind: "done"; usage: CliUsage; stopReason: string; costUsd: number }
  | { kind: "error"; message: string; rateLimited: boolean; resetAt?: number };

function buildArgs(opts: RunOptions): string[] {
  const args = [
    "--print",
    "--dangerously-skip-permissions",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--model",
    opts.model,
  ];
  if (opts.appendSystemPrompt) {
    args.push("--append-system-prompt", opts.appendSystemPrompt);
  }
  if (opts.sessionId) {
    args.push("--session-id", opts.sessionId);
  }
  return args;
}

const RATE_LIMIT_HINTS = [
  "rate limit",
  "rate-limit",
  "usage limit",
  "limit reached",
  "limit will reset",
  "resets at",
  "too many requests",
  "429",
];

function detectRateLimit(text: string): { rateLimited: boolean; resetAt?: number } {
  const lower = text.toLowerCase();
  const hit = RATE_LIMIT_HINTS.some((h) => lower.includes(h));
  if (!hit) return { rateLimited: false };

  // Try to recover a reset time: unix seconds, unix ms, or ISO timestamp.
  let resetAt: number | undefined;
  const epoch = text.match(/(?:resets?|reset at|until)[^0-9]*(\d{10,13})/i);
  if (epoch?.[1]) {
    const n = Number.parseInt(epoch[1], 10);
    resetAt = epoch[1].length >= 13 ? n : n * 1000;
  } else {
    const iso = text.match(/(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2})?[^\s"]*)/);
    if (iso?.[1]) {
      const t = Date.parse(iso[1]);
      if (Number.isFinite(t)) resetAt = t;
    }
  }
  return { rateLimited: true, resetAt };
}

/**
 * Run one turn and yield normalized events. Consumes the CLI's JSON stream.
 */
export async function* runClaude(
  prompt: string,
  opts: RunOptions,
): AsyncGenerator<TurnEvent, void, void> {
  const controller = new AbortController();
  const abort = () => controller.abort();
  opts.signal?.addEventListener("abort", abort, { once: true });

  const timeout = setTimeout(() => controller.abort(), opts.timeoutMs);

  let proc: Bun.Subprocess;
  try {
    proc = Bun.spawn([opts.claudeBin, ...buildArgs(opts)], {
      cwd: process.cwd(),
      // Strip vars that would break account isolation:
      //  - CLAUDECODE: so the CLI doesn't think it's nested inside itself.
      //  - ANTHROPIC_BASE_URL / _AUTH_TOKEN / _API_KEY: so the pooled account
      //    authenticates via ITS OWN login (config dir), never an injected
      //    endpoint — this also prevents a loop back into this very proxy.
      env: {
        ...Object.fromEntries(
          Object.entries(process.env).filter(
            ([k]) =>
              k !== "CLAUDECODE" &&
              k !== "ANTHROPIC_BASE_URL" &&
              k !== "ANTHROPIC_AUTH_TOKEN" &&
              k !== "ANTHROPIC_API_KEY",
          ),
        ),
        CLAUDE_CONFIG_DIR: opts.configDir,
      },
      stdin: new Blob([prompt]),
      stdout: "pipe",
      stderr: "pipe",
      signal: controller.signal,
    });
  } catch (err) {
    clearTimeout(timeout);
    opts.signal?.removeEventListener("abort", abort);
    yield {
      kind: "error",
      message: `Failed to spawn Claude CLI: ${(err as Error).message}`,
      rateLimited: false,
    };
    return;
  }

  const decoder = new TextDecoder();
  let buffer = "";
  let sawText = false;
  let finalResult: CliResult | null = null;
  let inTextBlock = false;

  try {
    for await (const chunk of proc.stdout as ReadableStream<Uint8Array>) {
      buffer += decoder.decode(chunk, { stream: true });
      const lines = buffer.split("\n");
      buffer = lines.pop() ?? "";

      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed) continue;

        let msg: CliMessage;
        try {
          msg = JSON.parse(trimmed) as CliMessage;
        } catch {
          continue; // Non-JSON diagnostic line.
        }

        if (isStreamEvent(msg)) {
          const ev = msg.event;
          if (ev.type === "content_block_start") {
            const cb = ev.content_block;
            if (cb?.type === "text") {
              if (sawText) yield { kind: "text_block_boundary" };
              inTextBlock = true;
            } else if (cb?.type === "tool_use") {
              inTextBlock = false;
              yield { kind: "tool_use", id: cb.id as string, name: cb.name as string };
            }
          } else if (ev.type === "content_block_delta") {
            const d = ev.delta;
            if (d?.type === "text_delta" && typeof (d as { text?: string }).text === "string") {
              sawText = true;
              yield { kind: "text", text: (d as { text: string }).text };
            }
          } else if (ev.type === "content_block_stop") {
            inTextBlock = false;
          }
        } else if (isResult(msg)) {
          finalResult = msg;
        } else if (isAssistant(msg)) {
          // Non-partial mode fallback: surface any text blocks not already streamed.
          if (!sawText) {
            for (const block of msg.message.content) {
              if (block.type === "text" && block.text) {
                yield { kind: "text", text: block.text };
                sawText = true;
              }
            }
          }
        }
      }
    }

    void inTextBlock;

    // Drain stderr for error diagnostics (rate limits often land here).
    let stderrText = "";
    try {
      stderrText = await new Response(proc.stderr as ReadableStream<Uint8Array>).text();
    } catch {}

    await proc.exited.catch(() => {});

    if (finalResult) {
      if (finalResult.is_error || finalResult.subtype === "error") {
        const text = `${finalResult.result || ""}\n${stderrText}`.trim();
        const rl = detectRateLimit(text);
        yield {
          kind: "error",
          message: finalResult.result || stderrText || "Claude CLI reported an error",
          rateLimited: rl.rateLimited,
          resetAt: rl.resetAt,
        };
        return;
      }
      // Some builds only put the final text in the result (no partial deltas).
      if (!sawText && finalResult.result) {
        yield { kind: "text", text: finalResult.result };
      }
      yield {
        kind: "done",
        usage: finalResult.usage,
        stopReason: "end_turn",
        costUsd: finalResult.total_cost_usd ?? 0,
      };
      return;
    }

    // No result line at all — treat as an error, checking stderr for rate limits.
    const rl = detectRateLimit(stderrText);
    yield {
      kind: "error",
      message: stderrText || "Claude CLI exited without producing a result",
      rateLimited: rl.rateLimited,
      resetAt: rl.resetAt,
    };
  } catch (err) {
    const aborted = controller.signal.aborted;
    yield {
      kind: "error",
      message: aborted
        ? "Request aborted or timed out"
        : `Stream error: ${(err as Error).message}`,
      rateLimited: false,
    };
  } finally {
    // Always run — including when the consumer abandons this generator early
    // (e.g. failover .return()). Guarantees the subprocess is not orphaned.
    clearTimeout(timeout);
    opts.signal?.removeEventListener("abort", abort);
    try {
      if (proc.exitCode === null) proc.kill();
    } catch {}
  }
}
