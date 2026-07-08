import { expect, test } from "bun:test";
import { createHash } from "crypto";
import {
  DEFAULT_KEYCHAIN_SERVICE,
  keychainServiceForConfigDir,
} from "./keychain.ts";

test("derives the sha256[:8]-suffixed service name Claude Code uses", () => {
  // Pinned vectors: `Claude Code-credentials-<sha256(configDir)[:8]>`.
  expect(keychainServiceForConfigDir("/home/user/.claude-max-pool/accounts/work")).toBe(
    "Claude Code-credentials-c34f05ad",
  );
  expect(keychainServiceForConfigDir("/Users/alice/.claude-max-pool/accounts/personal")).toBe(
    "Claude Code-credentials-f37fdcf4",
  );
});

test("service name matches an independently computed sha256 prefix", () => {
  const dir = "/some/pool/accounts/acme";
  const want = `${DEFAULT_KEYCHAIN_SERVICE}-${createHash("sha256")
    .update(dir)
    .digest("hex")
    .slice(0, 8)}`;
  expect(keychainServiceForConfigDir(dir)).toBe(want);
});
