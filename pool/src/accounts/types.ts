/** The `claudeAiOauth` block stored in an account's .credentials.json. */
export interface ClaudeOauthCreds {
  accessToken?: string;
  refreshToken?: string;
  expiresAt?: number;
  scopes?: string[];
  subscriptionType?: string;
  rateLimitTier?: string;
}

export interface CredentialsFile {
  claudeAiOauth?: ClaudeOauthCreds;
  [key: string]: unknown;
}

/** Rolling + lifetime usage counters for one account. */
export interface AccountUsage {
  /** Start of the current rolling window (epoch ms). */
  windowStart: number;
  windowRequests: number;
  windowInputTokens: number;
  windowOutputTokens: number;
  windowCostUsd: number;

  totalRequests: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCostUsd: number;

  lastUsedAt: number | null;
  lastError: string | null;

  /** If set and in the future, the account is sidelined until this time. */
  rateLimitedUntil: number | null;
}

/** Fully-resolved view of an account for status/routing. */
export interface Account {
  name: string;
  configDir: string;
  authenticated: boolean;
  subscriptionType: string | null;
  rateLimitTier: string | null;
  scopes: string[];
  tokenExpiresAt: number | null;
  tokenExpired: boolean;
  usage: AccountUsage;
  /** Available to serve traffic right now. */
  available: boolean;
  /** Human-readable reason when not available. */
  unavailableReason: string | null;
}

export function emptyUsage(now: number): AccountUsage {
  return {
    windowStart: now,
    windowRequests: 0,
    windowInputTokens: 0,
    windowOutputTokens: 0,
    windowCostUsd: 0,
    totalRequests: 0,
    totalInputTokens: 0,
    totalOutputTokens: 0,
    totalCostUsd: 0,
    lastUsedAt: null,
    lastError: null,
    rateLimitedUntil: null,
  };
}
