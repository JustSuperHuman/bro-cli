/**
 * macOS Keychain access for Claude Code OAuth credentials.
 *
 * On macOS the `claude` CLI stores credentials in the login Keychain, not in a
 * `$CLAUDE_CONFIG_DIR/.credentials.json` file — that plaintext file only exists
 * on Linux and Windows. The generic-password item is namespaced per config
 * directory: the default `~/.claude` uses the bare service name
 * `Claude Code-credentials`, and any other CLAUDE_CONFIG_DIR uses
 * `Claude Code-credentials-<sha256(configDir)[:8]>`.
 *
 * Each pooled account runs `claude` under its own CLAUDE_CONFIG_DIR, so every
 * account's login lands in its own Keychain item. Mirroring Claude Code's
 * naming scheme lets us read those items back for status and to seed the proxy.
 *
 * Verified against Claude Code 2.1.204: a login under
 * CLAUDE_CONFIG_DIR=<home>/.claude-max-pool/accounts/primary created the item
 * `Claude Code-credentials-28f0e4e1`, matching sha256(configDir)[:8].
 */
import { createHash } from "crypto";
import { execFileSync } from "child_process";
import type { CredentialsFile } from "./types.ts";

/** Service name for the default `~/.claude` config dir (unset CLAUDE_CONFIG_DIR). */
export const DEFAULT_KEYCHAIN_SERVICE = "Claude Code-credentials";

/** The Keychain generic-password service name Claude Code uses for a config dir. */
export function keychainServiceForConfigDir(configDir: string): string {
  const suffix = createHash("sha256").update(configDir).digest("hex").slice(0, 8);
  return `${DEFAULT_KEYCHAIN_SERVICE}-${suffix}`;
}

/**
 * Read + parse a Claude Code credentials blob from the macOS login Keychain by
 * service name. Returns null when the item is absent, unreadable, or not JSON.
 * Only meaningful on darwin — callers guard on process.platform.
 */
export function readKeychainCreds(service: string): CredentialsFile | null {
  let raw: string;
  try {
    raw = execFileSync("security", ["find-generic-password", "-s", service, "-w"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null; // item not found, or access to the Keychain was denied
  }
  try {
    return JSON.parse(raw.trim()) as CredentialsFile;
  } catch {
    return null;
  }
}

/**
 * Read Claude Code credentials for a given config dir from the Keychain. Tries
 * the per-dir service name first; for the default `~/.claude` dir, Claude Code
 * historically used the bare service name, so fall back to that.
 */
export function readKeychainCredsForConfigDir(configDir: string): CredentialsFile | null {
  return (
    readKeychainCreds(keychainServiceForConfigDir(configDir)) ??
    readKeychainCreds(DEFAULT_KEYCHAIN_SERVICE)
  );
}
