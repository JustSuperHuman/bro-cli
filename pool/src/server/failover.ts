/**
 * Account failover core.
 *
 * `runWithFailover` runs one turn and, if the serving account reports a
 * usage/rate limit *before any output has streamed*, transparently retries the
 * turn on the next available account. It is decoupled from the subprocess layer
 * via an injected event factory, which keeps it unit-testable.
 */

import type { AccountManager } from "../accounts/manager.ts";
import type { Account } from "../accounts/types.ts";
import type { TurnEvent } from "../subprocess/claude.ts";

/** Builds the normalized event stream for a turn on a given account. */
export type EventFactory = (account: Account) => AsyncGenerator<TurnEvent>;

/**
 * Pass-through generator that records usage/errors on the manager as the
 * normalized events flow to whichever adapter is consuming them.
 */
export async function* instrument(
  events: AsyncGenerator<TurnEvent>,
  mgr: AccountManager,
  accountName: string,
): AsyncGenerator<TurnEvent> {
  for await (const ev of events) {
    if (ev.kind === "done") {
      mgr.recordSuccess(accountName, ev.usage, ev.costUsd);
    } else if (ev.kind === "error") {
      if (ev.rateLimited) mgr.markRateLimited(accountName, ev.resetAt);
      else mgr.recordError(accountName, ev.message);
    }
    yield ev;
  }
}

export interface FailoverHooks {
  /** Called when a turn fails over from one account to another. */
  onFailover?: (from: string, to: string) => void;
}

/**
 * Runs a turn with graceful account failover. Starts on `first`; if an account
 * reports a usage/rate limit **before any output has streamed**, that account is
 * sidelined and the turn is transparently retried on the next available account.
 *
 * Once text has begun streaming we are committed to that account — bytes already
 * sent to the client can't be recalled — so only start-of-turn exhaustion fails
 * over (which is exactly the "usage ran out" case). Every attempted account is
 * excluded from subsequent picks, so we never loop on the same one.
 */
export async function* runWithFailover(
  mgr: AccountManager,
  sessionKey: string | undefined,
  first: Account,
  makeEvents: EventFactory,
  hooks: FailoverHooks = {},
): AsyncGenerator<TurnEvent> {
  const tried = new Set<string>();
  let account: Account | null = first;

  while (account) {
    tried.add(account.name);
    const events = instrument(makeEvents(account), mgr, account.name);

    let committed = false;
    let pendingRateLimit: Extract<TurnEvent, { kind: "error" }> | null = null;

    for await (const ev of events) {
      if (!committed && ev.kind === "error" && ev.rateLimited) {
        // No output sent yet — hold the error and try to fail over instead.
        pendingRateLimit = ev;
        break; // abandons `events`; its finally kills the subprocess
      }
      if (!committed) {
        committed = true;
        if (sessionKey) mgr.setAffinity(sessionKey, account.name);
      }
      yield ev;
    }

    if (!pendingRateLimit) return; // committed stream finished (success or hard error)

    const next = mgr.pick(sessionKey, tried);
    if (!next) {
      // No other account can take over — surface the rate-limit error as-is.
      yield pendingRateLimit;
      return;
    }
    hooks.onFailover?.(account.name, next.name);
    account = next;
  }
}
