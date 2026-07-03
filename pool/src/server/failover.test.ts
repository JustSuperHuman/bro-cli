import { test, expect } from "bun:test";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import { loadConfig } from "../config.ts";
import { AccountManager } from "../accounts/manager.ts";
import { runWithFailover, type EventFactory } from "./failover.ts";
import type { TurnEvent } from "../subprocess/claude.ts";

function tempPool(accountNames: string[]): { poolDir: string; mgr: AccountManager } {
  const poolDir = mkdtempSync(join(tmpdir(), "cmp-failover-"));
  const accountsDir = join(poolDir, "accounts");
  for (const name of accountNames) {
    const dir = join(accountsDir, name);
    mkdirSync(dir, { recursive: true });
    writeFileSync(
      join(dir, ".credentials.json"),
      JSON.stringify({
        claudeAiOauth: {
          accessToken: "tok-" + name,
          refreshToken: "r",
          expiresAt: Date.now() + 3_600_000,
          subscriptionType: "max",
          rateLimitTier: "default_claude_max_5x",
        },
      }),
    );
  }
  const config = loadConfig({ poolDir, accountsDir, usageFile: join(poolDir, "usage.json") });
  return { poolDir, mgr: new AccountManager(config) };
}

async function collect(gen: AsyncGenerator<TurnEvent>): Promise<TurnEvent[]> {
  const out: TurnEvent[] = [];
  for await (const ev of gen) out.push(ev);
  return out;
}

const exhausted = (): AsyncGenerator<TurnEvent> =>
  (async function* () {
    yield { kind: "error", message: "Claude usage limit reached", rateLimited: true };
  })();

const ok = (text: string): AsyncGenerator<TurnEvent> =>
  (async function* () {
    yield { kind: "text", text };
    yield { kind: "done", usage: { input_tokens: 5, output_tokens: 2 }, stopReason: "end_turn", costUsd: 0 };
  })();

test("fails over to the next account when the first is exhausted", async () => {
  const { poolDir, mgr } = tempPool(["a-exhausted", "b-ok"]);
  try {
    const factory: EventFactory = (a) => (a.name === "a-exhausted" ? exhausted() : ok("FAILOVER_OK"));
    const first = mgr.pick();
    expect(first?.name).toBe("a-exhausted"); // alphabetical, both idle -> first pick

    const failovers: string[] = [];
    const events = await collect(
      runWithFailover(mgr, undefined, first!, factory, {
        onFailover: (from, to) => failovers.push(`${from}->${to}`),
      }),
    );

    const text = events.filter((e) => e.kind === "text").map((e) => (e as any).text).join("");
    expect(text).toBe("FAILOVER_OK");
    expect(events.some((e) => e.kind === "done")).toBe(true);
    expect(events.some((e) => e.kind === "error")).toBe(false); // error was swallowed by failover
    expect(failovers).toEqual(["a-exhausted->b-ok"]);

    // The exhausted account is now sidelined; the healthy one served the turn.
    expect(mgr.getAccount("a-exhausted").available).toBe(false);
    expect(mgr.getAccount("b-ok").usage.totalRequests).toBe(1);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});

test("surfaces the rate-limit error when every account is exhausted", async () => {
  const { poolDir, mgr } = tempPool(["a-exhausted", "b-exhausted"]);
  try {
    const factory: EventFactory = () => exhausted();
    const first = mgr.pick();
    const events = await collect(runWithFailover(mgr, undefined, first!, factory));

    const err = events.find((e) => e.kind === "error");
    expect(err).toBeDefined();
    expect((err as any).rateLimited).toBe(true);
    expect(events.some((e) => e.kind === "text")).toBe(false);

    // Both accounts got sidelined.
    expect(mgr.getAccount("a-exhausted").available).toBe(false);
    expect(mgr.getAccount("b-exhausted").available).toBe(false);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});

test("does not fail over once output has started streaming", async () => {
  const { poolDir, mgr } = tempPool(["a-first", "b-second"]);
  try {
    // a-first streams text, THEN errors (not rate-limited) — must not reroute.
    const partialThenError = (): AsyncGenerator<TurnEvent> =>
      (async function* () {
        yield { kind: "text", text: "partial" };
        yield { kind: "error", message: "boom", rateLimited: false };
      })();
    const factory: EventFactory = (a) => (a.name === "a-first" ? partialThenError() : ok("SHOULD_NOT_SEE"));

    const first = mgr.pick();
    expect(first?.name).toBe("a-first");
    const events = await collect(runWithFailover(mgr, undefined, first!, factory));

    const text = events.filter((e) => e.kind === "text").map((e) => (e as any).text).join("");
    expect(text).toBe("partial"); // committed to a-first; b-second never used
    expect(events.some((e) => e.kind === "error")).toBe(true);
  } finally {
    rmSync(poolDir, { recursive: true, force: true });
  }
});
