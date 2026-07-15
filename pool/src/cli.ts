/**
 * `accounts` sub-commands for managing the pool from the terminal.
 *
 *   accounts list                 List accounts and their status.
 *   accounts login <name>         Create the account (if needed) and open an
 *                                 interactive Claude login in its own config dir.
 *   accounts import <name>        Copy the machine's current Claude login into a
 *                                 new pool account.
 *   accounts add <name>           Create an empty account dir (login separately).
 *   accounts remove <name>        Delete an account and its credentials.
 */

import type { Config } from "./config.ts";
import { AccountManager } from "./accounts/manager.ts";

function fmtWhen(ts: number | null): string {
  if (!ts) return "never";
  return new Date(ts).toLocaleString();
}

export async function runAccountsCommand(config: Config, args: string[]): Promise<number> {
  const mgr = new AccountManager(config);
  const [sub, name] = args;

  switch (sub) {
    case undefined:
    case "list": {
      const accounts = mgr.listAccounts();
      if (accounts.length === 0) {
        console.log(`No accounts yet.

Each account is a separate Claude Max / Team login the proxy can pool.

  1. Install the Claude CLI (if needed):
       npm install -g @anthropic-ai/claude-code

  2. Log in your first plan (run /login, then /exit inside the CLI):
       bun run src/index.ts accounts login work

     …or import the login already on this machine:
       bun run src/index.ts accounts import primary

  3. Repeat for each plan, then start the server:
       bun start

Pool dir: ${config.accountsDir}`);
        return 0;
      }
      console.log(`Pool dir: ${config.accountsDir}\n`);
      for (const a of accounts) {
        const state = a.available ? "READY" : a.authenticated ? "SIDELINED" : "LOGGED OUT";
        console.log(`● ${a.name}  [${state}]`);
        console.log(`    plan:      ${a.subscriptionType ?? "unknown"}   tier: ${a.rateLimitTier ?? "-"}`);
        console.log(
          `    token:     ${a.tokenExpired ? "expired (auto-refreshes on use)" : "valid until " + fmtWhen(a.tokenExpiresAt)}`,
        );
        console.log(
          `    usage:     ${a.usage.windowRequests} req / ${a.usage.windowInputTokens + a.usage.windowOutputTokens} tok this window · ${a.usage.totalRequests} req all-time`,
        );
        if (a.unavailableReason) console.log(`    note:      ${a.unavailableReason}`);
        if (a.usage.lastError) console.log(`    last err:  ${a.usage.lastError}`);
        console.log("");
      }
      return 0;
    }

    case "add": {
      if (!name) return usageErr("accounts add <name>");
      mgr.create(name);
      console.log(`Created account "${name}" at ${mgr.configDirFor(name)}`);
      console.log(`Now log in:  bun run src/index.ts accounts login ${name}`);
      return 0;
    }

    case "login": {
      if (!name) return usageErr("accounts login <name>");
      if (!mgr.listNames().includes(name)) mgr.create(name);
      const dir = mgr.configDirFor(name);
      console.log(`Opening interactive Claude login for "${name}".`);
      console.log(`Config dir: ${dir}`);
      console.log(`When Claude starts, run /login (or complete onboarding), then /exit.\n`);

      const proc = Bun.spawn([config.claudeBin], {
        cwd: process.cwd(),
        env: {
          ...Object.fromEntries(
            Object.entries(process.env).filter(([k]) => k !== "CLAUDECODE"),
          ),
          CLAUDE_CONFIG_DIR: dir,
        },
        stdin: "inherit",
        stdout: "inherit",
        stderr: "inherit",
      });
      await proc.exited;

      // macOS writes the login to the shared Keychain, not the account dir —
      // snapshot it into this account's file so the pool can read it.
      mgr.captureKeychainInto(name);

      const acct = mgr.getAccount(name);
      if (acct.authenticated) {
        console.log(`\n✓ "${name}" is authenticated (${acct.subscriptionType ?? "plan unknown"}, tier ${acct.rateLimitTier ?? "-"}).`);
        return 0;
      }
      console.log(`\n⚠ "${name}" still has no stored credentials. Re-run login and complete /login.`);
      return 1;
    }

    case "import": {
      if (!name) return usageErr("accounts import <name>");
      mgr.importCurrent(name);
      const acct = mgr.getAccount(name);
      console.log(
        acct.authenticated
          ? `✓ Imported current login into "${name}" (${acct.subscriptionType ?? "plan unknown"}).`
          : `Imported into "${name}", but no valid credentials were found.`,
      );
      return acct.authenticated ? 0 : 1;
    }

    case "remove":
    case "rm": {
      if (!name) return usageErr("accounts remove <name>");
      mgr.remove(name);
      console.log(`Removed account "${name}".`);
      return 0;
    }

    default:
      console.error(`Unknown accounts sub-command: ${sub}`);
      return usageErr("accounts <list|login|import|add|remove> [name]");
  }
}

function usageErr(usage: string): number {
  console.error(`Usage: bun run src/index.ts ${usage}`);
  return 1;
}
