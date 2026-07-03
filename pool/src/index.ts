#!/usr/bin/env bun
/**
 * Entry point. Dispatches between the HTTP server and the `accounts` CLI.
 *
 *   bun run src/index.ts serve [--port N] [--host H]
 *   bun run src/index.ts accounts <list|login|import|add|remove> [name]
 */

import { loadConfig, type Config } from "./config.ts";
import { startServer } from "./server/server.ts";
import { runAccountsCommand } from "./cli.ts";

function parseServeFlags(args: string[]): Partial<Config> {
  const overrides: Partial<Config> = {};
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === "--port" || a === "-p") {
      const v = args[++i];
      if (v) overrides.port = Number.parseInt(v, 10);
    } else if (a === "--host") {
      const v = args[++i];
      if (v) overrides.host = v;
    } else if (/^\d+$/.test(a ?? "")) {
      overrides.port = Number.parseInt(a!, 10);
    }
  }
  return overrides;
}

function printHelp(): void {
  console.log(`Claude Max Pool — pool many Claude plans behind one API endpoint.

Usage:
  bun run src/index.ts serve [--host H] [--port N]
  bun run src/index.ts accounts list
  bun run src/index.ts accounts login <name>
  bun run src/index.ts accounts import <name>
  bun run src/index.ts accounts add <name>
  bun run src/index.ts accounts remove <name>

Environment:
  CLAUDE_POOL_DIR     Pool directory (default ~/.claude-max-pool)
  CLAUDE_POOL_BACKEND oauth (default direct API) or cli (legacy subprocess)
  CLAUDE_BIN          Path to the claude executable (default "claude")
  ANTHROPIC_API_BASE_URL Direct backend API base (default https://api.anthropic.com)
  HOST / PORT         Bind address (default 127.0.0.1:3456)
  PROXY_API_KEY       Require this bearer token on /v1/* (default: none)
`);
}

async function main(): Promise<number> {
  const [command, ...rest] = Bun.argv.slice(2);

  if (command === "help" || command === "--help" || command === "-h") {
    printHelp();
    return 0;
  }

  if (command === "accounts") {
    const config = loadConfig();
    return runAccountsCommand(config, rest);
  }

  if (command === "serve" || command === undefined) {
    const config = loadConfig(parseServeFlags(rest));
    startServer(config);
    return 0; // server keeps the process alive
  }

  console.error(`Unknown command: ${command}\n`);
  printHelp();
  return 1;
}

const code = await main();
if (code !== 0) process.exit(code);
