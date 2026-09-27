// ChatJimmy — Taalas' free demo of Llama 3.1 8B etched into silicon (chatjimmy.ai), as a bro provider.
//
// chatjimmy.ai has no API: bro serves one. A small Anthropic-compatible endpoint (src/chatjimmy/)
// runs inside this process on a private loopback port, rewrites every request to fit the model's
// ~6.1k-token window, emulates tool calls, and is closed when the harness exits.
//
// Claude Code is launched lean so there is as little as possible to fit: no MCP servers, no skills,
// no settings-file hooks/plugins, no browser, six core tools, and a one-line system prompt (the
// endpoint writes the real, compact one).
//
// Terms: this drives the chatjimmy.ai web demo's backend. Taalas sells keyed API access
// separately; this route is for trying the model, not for production work.

import os from 'node:os';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { which, globalBinDirs, runInherit, ensureClaude } from './proc.js';
import { launchOmp, launchPi, permissionArgs } from './launch.js';
import { launchDsh } from './deepseek.js';
import { note } from './out.js';
import { createAnthropicHandler, JEV_MODEL_SUFFIX } from './chatjimmy/anthropic.js';
import { serveFetch } from './chatjimmy/server.js';
import { JIMMY_MODEL } from './chatjimmy/client.js';
import { CLAUDE_SYSTEM_PROMPT } from './chatjimmy/prompt.js';
import { createJevRouter, jevCredentials } from './chatjimmy/router.js';
import { jevKeyStatus } from './jev.js';
import { loadConfig } from './config.js';

export const JIMMY_JEV_MODEL = JIMMY_MODEL + JEV_MODEL_SUFFIX;

export const CHATJIMMY_PROVIDER = {
  id: 'chatjimmy',
  name: 'ChatJimmy (Llama 3.1 8B on Taalas silicon)',
  mode: 'chatjimmy',
  noKey: true,
  models: [
    { id: JIMMY_MODEL, name: 'Llama 3.1 8B · Taalas HC1 · free demo · ~6k context' },
    { id: JIMMY_JEV_MODEL, name: 'Llama 3.1 8B + Jev picking each step (TypeSafe or OpenRouter key)' }
  ]
};

/** Jev credentials for llama3.1-8B+jev: bro's Jev Router (TypeSafe) key, else an OpenRouter key. */
export function chatJimmyJevCredentials({ env = process.env, config = loadConfig() } = {}) {
  const typesafe = jevKeyStatus({ env, config });
  return jevCredentials({
    typesafeKey: typesafe.found ? typesafe.key : null,
    openrouterKey: config.keys?.openrouter || env.OPENROUTER_API_KEY || null
  });
}

// Claude Code's built-in tools that the endpoint describes in one line each (see chatjimmy/tools.js).
export const CHATJIMMY_TOOLS = 'Read,Write,Edit,Bash,Glob,Grep';
export const CHATJIMMY_LOG = path.join(os.homedir(), '.bro', 'chatjimmy.log');

/** Claude Code flags that keep the request small enough for ChatJimmy. */
export function leanClaudeArgs() {
  return [
    '--strict-mcp-config',        // no MCP servers (their tool lists alone overflow the window)
    '--disable-slash-commands',   // no skills
    '--no-chrome',                // no browser integration
    '--setting-sources', '',      // no user/project/local settings: hooks, plugins, extra env
    '--tools', CHATJIMMY_TOOLS,
    '--system-prompt', CLAUDE_SYSTEM_PROMPT
  ];
}

/** Environment for a harness pointed at the local endpoint. */
export function chatJimmyEnv(baseUrl, token, model = JIMMY_MODEL, base = process.env) {
  const env = { ...base };
  delete env.ANTHROPIC_API_KEY;
  env.ANTHROPIC_BASE_URL = baseUrl;
  env.ANTHROPIC_AUTH_TOKEN = token;
  env.ANTHROPIC_MODEL = model;
  for (const k of ['ANTHROPIC_DEFAULT_OPUS_MODEL', 'ANTHROPIC_DEFAULT_SONNET_MODEL', 'ANTHROPIC_DEFAULT_HAIKU_MODEL', 'ANTHROPIC_SMALL_FAST_MODEL', 'CLAUDE_CODE_SUBAGENT_MODEL']) env[k] = model;
  env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = '1';
  env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
  env.DISABLE_PROMPT_CACHING = '1';
  env.NODE_NO_WARNINGS = '1';
  return env;
}

const harnessProvider = (baseUrl) => ({ ...CHATJIMMY_PROVIDER, mode: 'anthropic', baseUrl, disable1mContext: true });

/** Start the endpoint in this process. `jev` credentials turn on Jev step routing. */
export async function startChatJimmy({ trace = process.env.BRO_CHATJIMMY_TRACE || null, jev = null } = {}) {
  const token = 'cj-' + randomBytes(16).toString('hex');
  const handler = createAnthropicHandler({
    token,
    logFile: CHATJIMMY_LOG,
    traceDir: trace,
    budget: Number(process.env.BRO_CHATJIMMY_BUDGET) || undefined,
    router: jev ? createJevRouter({ credentials: jev }) : null
  });
  const server = await serveFetch(handler);
  return { ...server, token };
}

export async function runChatJimmy({
  model = '',
  extraArgs = [],
  permissionMode = 'auto',
  harness = 'claude',
  providers = [],
  providerKeys = {},
  headless = false,
  dryRun = false
} = {}) {
  model = model || JIMMY_MODEL;
  if (harness === 'codex') {
    console.error('The codex harness can\'t use ChatJimmy: bro serves it over the Anthropic Messages API, and codex only talks to OpenAI\'s Responses API.');
    console.error('  Use claude, omp, Pi, or DeepSeek Harness (press h in the picker, or pass --claude / --omp / --pi / --dsh).');
    return 1;
  }

  const claudeArgs = [...permissionArgs(permissionMode), '--model', model, ...leanClaudeArgs(), ...extraArgs];
  if (dryRun) {
    const baseUrl = 'http://127.0.0.1:<ephemeral>';
    if (harness !== 'claude') {
      const provider = harnessProvider(baseUrl);
      const common = { provider, model, apiKey: '<per-launch token>', extraArgs, dryRun: true };
      return {
        via: 'ChatJimmy endpoint (in-process)',
        [harness]: harness === 'omp' ? await launchOmp({ ...common, skipPermissions: permissionMode === 'bypass' })
          : harness === 'pi' ? await launchPi(common)
          : await launchDsh({ ...common, providers: [provider, ...providers], providerKeys: { ...providerKeys, [provider.id]: '<per-launch token>' }, skipPermissions: permissionMode === 'bypass' })
      };
    }
    return {
      via: 'ChatJimmy endpoint (in-process, Anthropic Messages API)',
      baseUrl,
      cmd: which('claude', globalBinDirs()) || 'claude',
      args: claudeArgs,
      log: CHATJIMMY_LOG
    };
  }

  let jev = null;
  if (model.endsWith(JEV_MODEL_SUFFIX)) {
    jev = chatJimmyJevCredentials();
    if (!jev) {
      note('\x1b[2mllama3.1-8B+jev needs a TypeSafe key (JEV_API_KEY, or the key bro saved for Jev Router) or an OpenRouter key');
      note('  (OPENROUTER_API_KEY, or "openrouter" under keys in ~/.bro/config.json). Running ChatJimmy without Jev.\x1b[0m');
    }
  }
  const server = await startChatJimmy({ jev });
  note(`\nChatJimmy endpoint on ${server.url}${jev ? ` · Jev routing via ${jev.kind === 'typesafe' ? 'TypeSafe' : 'OpenRouter'}` : ''} (log: ${CHATJIMMY_LOG})`);
  try {
    if (harness === 'omp' || harness === 'pi' || harness === 'dsh') {
      const provider = harnessProvider(server.url);
      if (harness === 'omp') return await launchOmp({ provider, model, apiKey: server.token, extraArgs, skipPermissions: permissionMode === 'bypass' });
      if (harness === 'pi') return await launchPi({ provider, model, apiKey: server.token, extraArgs });
      return await launchDsh({
        provider, model, apiKey: server.token, extraArgs,
        providers: [provider, ...providers],
        providerKeys: { ...providerKeys, [provider.id]: server.token },
        skipPermissions: permissionMode === 'bypass'
      });
    }
    const { claude, dirs } = ensureClaude();
    const env = chatJimmyEnv(server.url, server.token, model);
    env.PATH = [...dirs, env.PATH || ''].join(path.delimiter);
    if (!headless) note('Claude Code runs lean here: no MCP, skills, hooks or browser; tools Read/Write/Edit/Bash/Glob/Grep.');
    note(`Launching Claude Code on ChatJimmy / ${model}…`);
    return await runInherit(claude, claudeArgs, env, { terminalAgent: 'claude' });
  } finally {
    await server.close();
  }
}
