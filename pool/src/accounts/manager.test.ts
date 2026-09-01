import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

import type { Config } from "../config.ts";
import { AccountManager } from "./manager.ts";

const roots: string[] = [];

afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function directConfig(): { config: Config; directDir: string } {
  const root = mkdtempSync(join(tmpdir(), "bro-direct-claude-test-"));
  roots.push(root);
  const directDir = join(root, "claude");
  mkdirSync(directDir, { recursive: true });
  writeFileSync(
    join(directDir, ".credentials.json"),
    JSON.stringify({
      preserved: true,
      claudeAiOauth: {
        accessToken: "sk-ant-oat-test",
        refreshToken: "refresh-test",
        expiresAt: Date.now() + 60_000,
        subscriptionType: "max",
      },
    }),
  );
  return {
    directDir,
    config: {
      poolDir: root,
      accountsDir: join(root, "accounts"),
      directClaudeConfigDir: directDir,
      directClaudeAccountName: "this-machine",
      usageFile: join(root, "usage.json"),
      claudeBin: "claude",
      backend: "oauth",
      anthropicApiBaseUrl: "https://api.anthropic.com",
      oauthTokenUrl: "https://platform.claude.com/v1/oauth/token",
      oauthClientId: "test-client",
      tokenRefreshSkewMs: 5_000,
      host: "127.0.0.1",
      port: 0,
      proxyApiKey: "proxy-test",
      requestTimeoutMs: 60_000,
      usageWindowMs: 60_000,
      rateLimitCooldownMs: 60_000,
      logFailover: false,
    },
  };
}

test("an existing Claude config is a live account, not a copied credential snapshot", () => {
  const { config, directDir } = directConfig();
  const manager = new AccountManager(config);

  expect(manager.listNames()).toEqual(["this-machine"]);
  expect(manager.getAccount("this-machine")).toMatchObject({
    configDir: directDir,
    authenticated: true,
    subscriptionType: "max",
    available: true,
  });

  manager.updateOAuthCreds("this-machine", {
    accessToken: "sk-ant-oat-rotated",
    refreshToken: "refresh-rotated",
    expiresAt: Date.now() + 120_000,
  });
  const stored = JSON.parse(readFileSync(join(directDir, ".credentials.json"), "utf8"));
  expect(stored.preserved).toBe(true);
  expect(stored.claudeAiOauth.accessToken).toBe("sk-ant-oat-rotated");
  expect(() => manager.remove("this-machine")).toThrow(/active Claude Code login/);
});
