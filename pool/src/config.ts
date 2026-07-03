/**
 * Central configuration, resolved once from the environment.
 *
 * The "pool directory" holds one sub-directory per Claude account. Each of those
 * sub-directories stores a Claude Code OAuth credential set. The direct backend
 * reads those tokens for upstream API calls; the CLI fallback also uses each
 * directory as CLAUDE_CONFIG_DIR when spawning Claude Code.
 */

import { homedir } from "os";
import { join } from "path";

export interface Config {
  /** Root directory that holds every account's config dir + pool state. */
  poolDir: string;
  /** Directory holding one sub-directory per account (each a CLAUDE_CONFIG_DIR). */
  accountsDir: string;
  /** File where rolling usage counters are persisted between restarts. */
  usageFile: string;
  /** Path to the `claude` executable. */
  claudeBin: string;
  /** Inference backend for /v1/messages: direct OAuth proxy by default, CLI as fallback. */
  backend: "oauth" | "cli";
  /** Base URL for the real Anthropic API when using the direct OAuth backend. */
  anthropicApiBaseUrl: string;
  /** API version sent upstream when the client did not provide one. */
  anthropicVersion: string;
  /** Beta header value that enables Claude Code OAuth bearer tokens upstream. */
  oauthBetaHeader: string;
  /** OAuth token endpoint used to refresh Claude Code account credentials. */
  oauthTokenUrl: string;
  /** Public Claude Code OAuth client id. Override if Anthropic changes it. */
  oauthClientId: string;
  /** Refresh access tokens this long before their recorded expiry. */
  tokenRefreshSkewMs: number;
  /** HTTP host to bind. */
  host: string;
  /** HTTP port to bind. */
  port: number;
  /** Optional bearer token required on /v1/* requests. Empty = no auth. */
  proxyApiKey: string;
  /** Per-request upstream/subprocess timeout in milliseconds. */
  requestTimeoutMs: number;
  /**
   * Length of the rolling usage window in milliseconds. Claude Max plans reset
   * usage roughly every 5 hours; we mirror that window for display/routing.
   */
  usageWindowMs: number;
  /** How long to sideline an account after it reports a rate limit, in ms. */
  rateLimitCooldownMs: number;
  /** Log a line when a request fails over from one account to another. */
  logFailover: boolean;
}

function intEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) return fallback;
  const n = Number.parseInt(raw, 10);
  return Number.isFinite(n) ? n : fallback;
}

function backendEnv(): "oauth" | "cli" {
  const raw = (process.env.CLAUDE_POOL_BACKEND || process.env.POOL_BACKEND || "oauth").toLowerCase();
  return raw === "cli" || raw === "subprocess" || raw === "claude" ? "cli" : "oauth";
}

export function loadConfig(overrides: Partial<Config> = {}): Config {
  const poolDir = process.env.CLAUDE_POOL_DIR || join(homedir(), ".claude-max-pool");
  const accountsDir = join(poolDir, "accounts");
  const usageFile = join(poolDir, "usage.json");

  const config: Config = {
    poolDir,
    accountsDir,
    usageFile,
    claudeBin: process.env.CLAUDE_BIN || "claude",
    backend: backendEnv(),
    anthropicApiBaseUrl: process.env.ANTHROPIC_API_BASE_URL || "https://api.anthropic.com",
    anthropicVersion: process.env.ANTHROPIC_VERSION || "2023-06-01",
    oauthBetaHeader: process.env.ANTHROPIC_OAUTH_BETA || "oauth-2025-04-20",
    oauthTokenUrl: process.env.CLAUDE_OAUTH_TOKEN_URL || "https://platform.claude.com/v1/oauth/token",
    oauthClientId: process.env.CLAUDE_OAUTH_CLIENT_ID || "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
    tokenRefreshSkewMs: intEnv("TOKEN_REFRESH_SKEW_MS", 5 * 60 * 1000),
    host: process.env.HOST || "127.0.0.1",
    port: intEnv("PORT", 3456),
    proxyApiKey: process.env.PROXY_API_KEY || "",
    requestTimeoutMs: intEnv("REQUEST_TIMEOUT_MS", 15 * 60 * 1000),
    usageWindowMs: intEnv("USAGE_WINDOW_MS", 5 * 60 * 60 * 1000),
    rateLimitCooldownMs: intEnv("RATE_LIMIT_COOLDOWN_MS", 60 * 60 * 1000),
    logFailover: process.env.LOG_FAILOVER !== "0",
    ...overrides,
  };

  return config;
}

/** Default config dir the Claude CLI uses when CLAUDE_CONFIG_DIR is unset. */
export function defaultClaudeConfigDir(): string {
  return process.env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
}
