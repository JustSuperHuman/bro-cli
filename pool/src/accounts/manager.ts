/**
 * AccountManager owns the pool of Claude accounts.
 *
 * Each account is a directory under <poolDir>/accounts/<name>/ with its own
 * Claude Code OAuth credentials. The manager reads each account's credentials
 * for status, tracks rolling usage, sidelines rate-limited accounts, and picks
 * which account should serve a given request (sticky by session, else
 * least-loaded).
 */

import { mkdirSync, existsSync, readdirSync, readFileSync, writeFileSync, rmSync, copyFileSync, statSync } from "fs";
import { join } from "path";
import type { Config } from "../config.ts";
import { defaultClaudeConfigDir } from "../config.ts";
import type { Account, AccountUsage, CredentialsFile } from "./types.ts";
import { emptyUsage } from "./types.ts";
import type { CliUsage } from "../subprocess/types.ts";

interface PersistedState {
  usage: Record<string, AccountUsage>;
}

/**
 * Read Claude Code's OAuth credentials from the macOS login Keychain, returning
 * the raw JSON string (same shape as .credentials.json) or null if unavailable.
 * No-op (null) on non-macOS platforms.
 */
function readMacKeychainCreds(): string | null {
  if (process.platform !== "darwin") return null;
  try {
    const proc = Bun.spawnSync([
      "security",
      "find-generic-password",
      "-s",
      "Claude Code-credentials",
      "-w",
    ]);
    if (proc.exitCode !== 0) return null;
    const out = proc.stdout.toString().trim();
    return out.startsWith("{") ? out : null;
  } catch {
    return null;
  }
}

export class AccountManager {
  private config: Config;
  private usage: Record<string, AccountUsage> = {};
  /** Maps a caller session key to the account chosen for it (stickiness). */
  private sessionAffinity = new Map<string, string>();
  /** Round-robin cursor for tie-breaking least-loaded selection. */
  private rrCursor = 0;

  constructor(config: Config) {
    this.config = config;
    mkdirSync(this.config.accountsDir, { recursive: true });
    this.loadState();
  }

  // ---- persistence -------------------------------------------------------

  private loadState(): void {
    if (!existsSync(this.config.usageFile)) return;
    try {
      const parsed = JSON.parse(readFileSync(this.config.usageFile, "utf8")) as PersistedState;
      this.usage = parsed.usage ?? {};
    } catch {
      this.usage = {};
    }
  }

  private saveState(): void {
    const state: PersistedState = { usage: this.usage };
    try {
      writeFileSync(this.config.usageFile, JSON.stringify(state, null, 2));
    } catch {
      // Non-fatal: usage stats are best-effort.
    }
  }

  private usageFor(name: string): AccountUsage {
    let u = this.usage[name];
    if (!u) {
      u = emptyUsage(Date.now());
      this.usage[name] = u;
    }
    this.rollWindow(u);
    return u;
  }

  private rollWindow(u: AccountUsage): void {
    const now = Date.now();
    if (now - u.windowStart >= this.config.usageWindowMs) {
      u.windowStart = now;
      u.windowRequests = 0;
      u.windowInputTokens = 0;
      u.windowOutputTokens = 0;
      u.windowCostUsd = 0;
    }
  }

  // ---- account directory management -------------------------------------

  /** Names of every account directory present in the pool. */
  listNames(): string[] {
    if (!existsSync(this.config.accountsDir)) return [];
    return readdirSync(this.config.accountsDir, { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name)
      .sort();
  }

  configDirFor(name: string): string {
    return join(this.config.accountsDir, name);
  }

  private credsPath(name: string): string {
    return join(this.configDirFor(name), ".credentials.json");
  }

  create(name: string): void {
    this.assertValidName(name);
    const dir = this.configDirFor(name);
    if (existsSync(dir)) throw new Error(`Account "${name}" already exists`);
    mkdirSync(dir, { recursive: true });
  }

  remove(name: string): void {
    const dir = this.configDirFor(name);
    if (!existsSync(dir)) throw new Error(`Account "${name}" does not exist`);
    rmSync(dir, { recursive: true, force: true });
    delete this.usage[name];
    this.saveState();
  }

  /** Copy the machine's current Claude login into a new pool account. */
  importCurrent(name: string): void {
    this.create(name);
    const src = join(defaultClaudeConfigDir(), ".credentials.json");
    if (existsSync(src)) {
      copyFileSync(src, this.credsPath(name));
      return;
    }
    // macOS stores the login in the Keychain rather than a file.
    const keychain = readMacKeychainCreds();
    if (keychain) {
      writeFileSync(this.credsPath(name), keychain);
      return;
    }
    throw new Error(
      process.platform === "darwin"
        ? `No Claude credentials found (looked at ${src} and the macOS Keychain). ` +
          `Log in with the base 'claude' CLI first, or use 'accounts login ${name}'.`
        : `No credentials found at ${src}. Log in with the base 'claude' CLI first, or use 'accounts login ${name}'.`,
    );
  }

  /**
   * macOS only: Claude Code writes OAuth credentials to a single shared login
   * Keychain entry instead of a per-CLAUDE_CONFIG_DIR file, so a fresh
   * `accounts login` leaves the account directory empty. Snapshot whatever is
   * currently in the Keychain into this account's credentials file so the pool
   * (which reads per-account files) can use it. No-op off macOS or when the
   * account already has a file. Returns true if it captured credentials.
   */
  captureKeychainInto(name: string): boolean {
    if (process.platform !== "darwin") return false;
    if (existsSync(this.credsPath(name))) return false;
    const keychain = readMacKeychainCreds();
    if (!keychain) return false;
    writeFileSync(this.credsPath(name), keychain);
    return true;
  }

  private assertValidName(name: string): void {
    if (!/^[A-Za-z0-9._-]{1,64}$/.test(name)) {
      throw new Error(
        `Invalid account name "${name}". Use letters, numbers, dot, dash, underscore (max 64 chars).`,
      );
    }
  }

  // ---- status ------------------------------------------------------------

  private readCreds(name: string): CredentialsFile | null {
    const p = this.credsPath(name);
    if (!existsSync(p)) return null;
    try {
      return JSON.parse(readFileSync(p, "utf8")) as CredentialsFile;
    } catch {
      return null;
    }
  }

  getOAuthCreds(name: string): CredentialsFile["claudeAiOauth"] | null {
    return this.readCreds(name)?.claudeAiOauth ?? null;
  }

  updateOAuthCreds(name: string, oauth: NonNullable<CredentialsFile["claudeAiOauth"]>): void {
    const existing = this.readCreds(name) ?? {};
    const next: CredentialsFile = { ...existing, claudeAiOauth: oauth };
    writeFileSync(this.credsPath(name), JSON.stringify(next, null, 2));
  }

  getAccount(name: string): Account {
    const creds = this.readCreds(name);
    const oauth = creds?.claudeAiOauth ?? null;
    const authenticated = Boolean(oauth?.accessToken);
    const tokenExpiresAt = oauth?.expiresAt ?? null;
    const tokenExpired = tokenExpiresAt != null && tokenExpiresAt < Date.now();

    const usage = this.usageFor(name);
    const now = Date.now();
    const cooling = usage.rateLimitedUntil != null && usage.rateLimitedUntil > now;

    let available = true;
    let reason: string | null = null;
    if (!authenticated) {
      available = false;
      reason = "not authenticated — run `accounts login`";
    } else if (cooling) {
      available = false;
      const mins = Math.ceil((usage.rateLimitedUntil! - now) / 60000);
      reason = `rate limited — retry in ~${mins} min`;
    }

    return {
      name,
      configDir: this.configDirFor(name),
      authenticated,
      subscriptionType: oauth?.subscriptionType ?? null,
      rateLimitTier: oauth?.rateLimitTier ?? null,
      scopes: oauth?.scopes ?? [],
      tokenExpiresAt,
      tokenExpired,
      usage,
      available,
      unavailableReason: reason,
    };
  }

  listAccounts(): Account[] {
    return this.listNames().map((n) => this.getAccount(n));
  }

  // ---- routing -----------------------------------------------------------

  /**
   * Pick an account to serve a request. Honors session affinity when the chosen
   * account is still available and not excluded; otherwise selects the
   * least-loaded available account (fewest requests in the current window),
   * round-robin on ties.
   *
   * @param exclude account names to skip (e.g. ones already tried this request
   *   during failover).
   */
  pick(sessionKey?: string, exclude?: ReadonlySet<string>): Account | null {
    if (sessionKey) {
      const prior = this.sessionAffinity.get(sessionKey);
      if (prior && !exclude?.has(prior)) {
        const acct = this.getAccount(prior);
        if (acct.available) return acct;
        this.sessionAffinity.delete(sessionKey);
      }
    }

    const available = this.listAccounts().filter(
      (a) => a.available && !exclude?.has(a.name),
    );
    if (available.length === 0) return null;

    let best = available[0]!;
    for (const a of available) {
      if (a.usage.windowRequests < best.usage.windowRequests) best = a;
    }
    // Round-robin among the accounts tied for the minimum load.
    const minLoad = best.usage.windowRequests;
    const tied = available.filter((a) => a.usage.windowRequests === minLoad);
    if (tied.length > 1) {
      best = tied[this.rrCursor % tied.length]!;
      this.rrCursor = (this.rrCursor + 1) % tied.length;
    }

    if (sessionKey) this.sessionAffinity.set(sessionKey, best.name);
    return best;
  }

  /** Pin a session to the account that actually served it (post-failover). */
  setAffinity(sessionKey: string, accountName: string): void {
    this.sessionAffinity.set(sessionKey, accountName);
  }

  // ---- usage recording ---------------------------------------------------

  recordSuccess(name: string, usage: CliUsage, costUsd: number): void {
    const u = this.usageFor(name);
    const now = Date.now();
    u.windowRequests += 1;
    u.windowInputTokens += usage.input_tokens ?? 0;
    u.windowOutputTokens += usage.output_tokens ?? 0;
    u.windowCostUsd += costUsd;
    u.totalRequests += 1;
    u.totalInputTokens += usage.input_tokens ?? 0;
    u.totalOutputTokens += usage.output_tokens ?? 0;
    u.totalCostUsd += costUsd;
    u.lastUsedAt = now;
    u.lastError = null;
    this.saveState();
  }

  recordError(name: string, message: string): void {
    const u = this.usageFor(name);
    u.lastError = message.slice(0, 500);
    u.lastUsedAt = Date.now();
    this.saveState();
  }

  markRateLimited(name: string, resetAt?: number): void {
    const u = this.usageFor(name);
    u.rateLimitedUntil = resetAt ?? Date.now() + this.config.rateLimitCooldownMs;
    u.lastError = "rate limited by Anthropic";
    // Drop affinity so sessions reroute away from this account.
    for (const [k, v] of this.sessionAffinity) if (v === name) this.sessionAffinity.delete(k);
    this.saveState();
  }

  clearRateLimit(name: string): void {
    const u = this.usageFor(name);
    u.rateLimitedUntil = null;
    this.saveState();
  }

  /** True if any account holds a valid-looking login. */
  hasUsableAccount(): boolean {
    return this.listAccounts().some((a) => a.authenticated);
  }

  poolMtime(): number {
    try {
      return statSync(this.config.accountsDir).mtimeMs;
    } catch {
      return 0;
    }
  }
}
