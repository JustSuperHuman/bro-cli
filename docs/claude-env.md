# Claude Code environment in bro

bro sets Claude Code environment variables on every Claude launch from one **preset** plus per-switch
**overrides**, chosen in the Settings view (the ⚙ in the sidebar, `alt+,`, prefix `A`, or "settings" in the palette). Presets can also be
switched from the palette ("claude preset …"). Changes apply to sessions launched afterwards.

Saved in `~/.bro/v2.toml`:

```toml
[claude_env]
preset = "private"

[claude_env.overrides]   # knob id = option id
telemetry = "default"

[claude_env.extra]       # any other variable, set last on every Claude launch
CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY = "16"
```

A provider's own `env` (in `~/.bro/config.json`) and bro's routing variables (`ANTHROPIC_BASE_URL`,
model slots, `CLAUDE_CODE_DISABLE_1M_CONTEXT`, …) always take priority over the preset.

Source: <https://code.claude.com/docs/en/env-vars> (checked 2026-10-11).

## Presets

| Preset | What it turns on | Cost |
|---|---|---|
| Standard | nothing | — |
| Quiet | no error reports or feedback prompts | none |
| Private | Quiet + no telemetry, prompt fingerprint or claude.ai connectors | No Remote Control, artifact comments, advisor tool or claude.ai skill sync, because telemetry off also stops feature-flag fetching |
| Lockdown | Private + no nonessential traffic, secrets scrubbed from tool shells, minimal MCP server env, no auto memory, manual updates | same as Private; MCP servers that read keys from your shell env need them in their own `env` config |
| Incognito | Quiet + no transcripts or history on disk, no auto memory | those sessions can't be continued, resumed or recapped |
| Presenter | Quiet + email, org and working directory hidden | none |
| Marathon | Quiet + retries overloads indefinitely, long shell timeouts, 1h prompt cache, manual updates | 1h cache writes cost 2x on API keys |

## Agent teams

[Agent teams](https://code.claude.com/docs/en/agent-teams) let a Claude session lead a team: you ask for teammates
("Spawn three teammates to review this branch: one on security, one on performance, one on tests"), Claude spawns
them, each its own Claude instance, and they share a task list and message each other. bro turns them **on** for every
Claude launch. They sit in a group of their own at the top of Settings › Claude Code, above the preset. No preset
touches them, and switching presets keeps what you picked:

| Switch | Options | Variables |
|---|---|---|
| Agent teams | **on** (bro's default) / off (Claude's default) | `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` |
| Teammate model | **lead's** / Opus / Sonnet / Haiku | `CLAUDE_CODE_SUBAGENT_MODEL`: what teammates and subagents run on when Claude doesn't name a model. A third-party backend pins every model slot, this one included. |

`r` on either switch goes back to bro's default. With agent teams on, a subagent that Claude names launches as a
teammate, so teams can form without being asked for. Turn the switch off if you'd rather keep subagents.

While a session leads a team, bro shows the team live, read from Claude Code's own files in the session's config dir
(`teams/session-<first 8 of its id>/config.json` and `tasks/<team>/`), once a second off the UI thread:

- **Sidebar**: under the lead's row, how many teammates are working and the tasks done, then each teammate with its
  status, its name in Claude's colour for it, and what it's doing. A click opens the team board.
- **Team board** (prefix `M`, "agent team" in the palette): the members, then the task list lane by lane (in
  progress, up next, blocked, done), each task with its owner, or what it waits on. A selected task shows its
  description. Enter goes to the lead; `s` opens these settings.
- **Pane header** (desktop app): "team · 2 of 3 working".
- **Settings**: the teams running now, under the switch.

To talk to one teammate, go to the lead and use Claude's agent panel (↑/↓ picks a teammate, enter opens it). Teammates
run in-process inside the lead's pane. Claude Code's split-pane mode needs tmux or iTerm2, which bro's terminals aren't.

## Built in: the Settings view's switches

| Switch | Variables |
|---|---|
| Error reports | `DISABLE_ERROR_REPORTING` |
| Feedback prompts | `CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`, `CLAUDE_CODE_SEND_FEEDBACK=0` |
| Prompt fingerprint | `CLAUDE_CODE_ATTRIBUTION_HEADER=0` |
| Usage telemetry | `DISABLE_TELEMETRY` |
| Nonessential traffic | `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`, `CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL` |
| claude.ai connectors | `ENABLE_CLAUDEAI_MCP_SERVERS=false` |
| Secrets in tool shells | `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` |
| MCP server env | `CLAUDE_CODE_MCP_ALLOWLIST_ENV` |
| Transcripts & history | `CLAUDE_CODE_SKIP_PROMPT_HISTORY` |
| Auto memory | `CLAUDE_CODE_DISABLE_AUTO_MEMORY` |
| Screen-share safe | `IS_DEMO`, `CLAUDE_CODE_HIDE_CWD` |
| Messages between sessions | bro's hooks on `SendMessage` and `ListAgents` (this project, the default); off adds `crossSessionInbound: "refuse"` and `isolatePeerMachines: true`. See [below](#messages-between-sessions). |
| Claude updates | `DISABLE_AUTOUPDATER` (manual) / `DISABLE_UPDATES` (frozen) |
| Overload errors | `CLAUDE_CODE_RETRY_WATCHDOG` |
| Shell command time limit | `BASH_DEFAULT_TIMEOUT_MS`, `BASH_MAX_TIMEOUT_MS` |
| Prompt cache lifetime | `CLAUDE_CODE_PROMPT_CACHE_TTL`, `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL` / `FORCE_PROMPT_CACHING_5M` |
| Auto-compact | `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` / `DISABLE_AUTO_COMPACT` |
| Tool search via proxy | `ENABLE_TOOL_SEARCH=true`, on by default for launches through bro-proxy to a Claude upstream (pool, Claude logins with compression, Anthropic providers). Claude Code otherwise turns tool search off behind any proxy and loads every MCP tool definition into context up front. |
| Claude renderer | `CLAUDE_CODE_NO_FLICKER` (fullscreen) / `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN` (classic) |

## Messages between sessions

Claude Code sessions on one machine can find each other with `ListAgents` and write to each other with
`SendMessage`, and Claude Code doesn't scope that by project. Every session on a login registers in that login's
`sessions` folder (`~/.claude/sessions`, or a pooled account's own config dir), and each one sees every other one
there. So a session in one repo can be told about, or interrupted by, work in an unrelated one. bro keeps them apart
with the **Messages between sessions** switch, under Settings › Claude Code › privacy:

| Option | What Claude can reach |
|---|---|
| this project (default) | Sessions in the same repository, and bro's orchestrator. Every worktree of a repo counts as the repo; a folder outside git is a project of its own. |
| off | No other session. Incoming messages are refused too, and a message bound for another machine waits for your approval. |
| Claude default | Every session on this machine, as Claude Code ships. |

Each Claude launch's `--settings` file gets two hooks that run bro itself (`bro hook peers <scope> <folder>`). They
use the exec form, so no shell sits in between. A `PreToolUse` hook on `SendMessage` denies a send to a session out of
reach, and Claude tells you what it meant to send instead. A `PostToolUse` hook on `ListAgents` takes those sessions
out of the listing Claude reads and adds a line saying how many it left out. Subagents, teammates and `main` aren't
sessions in the registry, so messages to them go through as before. Your own `/list-agents` still shows everything.

What it doesn't cover:

- Only sessions bro starts carry the hooks. A `claude` you run in another terminal can still message bro's sessions,
  and under "this project" they take it, because Claude Code can't filter incoming messages by sender. Under "off" they
  refuse it.
- Sessions on other machines and in the cloud (listed only while Remote Control is connected) aren't in the local
  registry. "This project" leaves them alone; "off" hides them and holds anything sent to them for your approval.
- Pass your own `--settings` and bro adds none, so no hooks either.
- The orchestrator's own launch gets no hooks, since its job spans every project.
- Exec-form hooks need Claude Code 2.1.139 or later. Cross-session messaging itself needs 2.1.224, or 2.1.239 on
  Windows.

## Already set by bro's launch code

`CLAUDE_CONFIG_DIR` (account profiles), `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY`
(providers and bro-proxy), `ANTHROPIC_MODEL`, `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`,
`ANTHROPIC_SMALL_FAST_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL` (pinned on third-party backends),
`CLAUDE_CODE_DISABLE_1M_CONTEXT` (non-Claude upstreams), `CLAUDE_CODE_ENABLE_CFC` (browser modes),
`MAX_THINKING_TOKENS` (bridge orchestrator), `CAVEMAN_DEFAULT_MODE=off` (caveman mode on; see [caveman.md](caveman.md)).
Claude Code keeps only the last `--append-system-prompt-file`, so bro merges every one it or you pass into one file. bro strips the session markers Claude Code sets in its own
subprocesses (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_PID`, …) so a bro
started from inside Claude Code doesn't launch "child" sessions.

## Ideas for later features

| Variable | Idea |
|---|---|
| `CLAUDE_CODE_TASK_LIST_ID` | "Linked panes": sessions in one bro project or split share one task list. |
| `CLAUDE_CODE_SHELL_PREFIX` | Command audit log: wrap every Bash/hook/MCP command with a bro logger and show it in a pane. |
| `CLAUDE_CLIENT_PRESENCE_FILE` | bro creates the file while its window is focused, so phone pushes only arrive when you're away. |
| `CLAUDE_ENV_FILE` | Per-project env (venv/conda/nvm) that bro writes and every Bash command sources. |
| `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY` | bro-proxy already serves `/r/<route>/v1/models`; this would fill Claude's `/model` picker on proxied routes. |
| `CLAUDE_CODE_MAX_CONTEXT_TOKENS`, `CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT` | Set the real context window per third-party model from the provider catalogue, so auto-compact runs at the right point. |
| `CLAUDE_CODE_ALWAYS_ENABLE_EFFORT` | Send effort to third-party models that support reasoning under custom ids. |
| `CLAUDE_CODE_EXTRA_BODY` | Provider-specific request fields (e.g. reasoning switches) from the provider catalogue. |
| `ANTHROPIC_CUSTOM_HEADERS` | Tag requests with the bro session id so the proxy log can show which pane sent each request. |
| `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS`, `DISABLE_INTERLEAVED_THINKING`, `CLAUDE_CODE_DISABLE_THINKING` | Per-provider compatibility flags for strict gateways (as provider `env` today). |
| `CLAUDE_CODE_EFFORT_LEVEL` | A per-launch effort picker in the launcher. Not a global switch: it overrides `/effort` mid-session. |
| `CLAUDE_REMOTE_CONTROL_SESSION_NAME_PREFIX` | Name Remote Control sessions after the bro session. |
| `CLAUDE_CODE_PLUGIN_DIRS` | Ship bro's own skills (bro-open, bro-browser) as a plugin dir instead of writing them into each config dir. |
| `CLAUDE_AFK_TIMEOUT_MS` | An "away" mode for unattended panes: unanswered questions auto-continue. |

## Situational: set through `[claude_env.extra]`

Limits and timeouts: `BASH_MAX_OUTPUT_LENGTH`, `MAX_MCP_OUTPUT_TOKENS`, `CLAUDE_CODE_FILE_READ_MAX_OUTPUT_TOKENS`,
`CLAUDE_CODE_MAX_OUTPUT_TOKENS`, `MAX_THINKING_TOKENS`, `CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY`,
`CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS`, `CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH`, `CLAUDE_CODE_MAX_WEB_SEARCHES_PER_SESSION`,
`CLAUDE_CODE_WEB_SEARCH_REFILLS_PER_HOUR`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, `API_TIMEOUT_MS`, `CLAUDE_CODE_MAX_RETRIES`,
`CLAUDE_CODE_RETRY_WATCHDOG_MAX_WAIT_MS`, `CLAUDE_CODE_OVERLOADED_RETRY_BASE_DELAY_MS`, `CLAUDE_STREAM_IDLE_TIMEOUT_MS`,
`API_FORCE_IDLE_TIMEOUT`, `MCP_TIMEOUT`, `MCP_TOOL_TIMEOUT`, `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`,
`CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS`, `CLAUDE_CODE_WEBFETCH_DEADLINE_MS`, `CLAUDE_CODE_GLOB_TIMEOUT_SECONDS`,
`SLASH_COMMAND_TOOL_CHAR_BUDGET`, `CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH`.

Features and behaviour: `CLAUDE_CODE_DISABLE_WEB_FETCH`, `CLAUDE_CODE_DISABLE_ARTIFACT`, `CLAUDE_CODE_ARTIFACT_AUTO_OPEN=0`,
`CLAUDE_CODE_DISABLE_CRON`, `CLAUDE_CODE_DISABLE_WORKFLOWS`, `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS`,
`CLAUDE_CODE_DISABLE_AGENT_VIEW`, `CLAUDE_CODE_DISABLE_ADVISOR_TOOL`, `CLAUDE_CODE_DISABLE_FAST_MODE`,
`CLAUDE_CODE_DISABLE_FILE_CHECKPOINTING`, `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS`, `CLAUDE_CODE_DISABLE_CLAUDE_MDS`,
`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`, `CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS`, `CLAUDE_CODE_DISABLE_BUNDLED_SKILLS`,
`CLAUDE_CODE_ENABLE_AWAY_SUMMARY`, `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false`, `CLAUDE_AUTO_BACKGROUND_TASKS`,
`CLAUDE_CODE_GOAL_CHECKIN_MINUTES`, `CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR`, `CLAUDE_CODE_BASH_EDIT_DIFF`,
`CLAUDE_CODE_GLOB_HIDDEN`, `CLAUDE_CODE_GLOB_NO_IGNORE`, `CLAUDE_CODE_NEW_INIT`, `CLAUDE_CODE_SAFE_MODE`,
`CLAUDE_CODE_SIMPLE`, `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT`, `CLAUDE_CODE_RESTRICTED`, `DISABLE_COMPACT`,
`DISABLE_COST_WARNINGS`, `DISABLE_GROWTHBOOK`, `DO_NOT_TRACK`, `DISABLE_PROMPT_CACHING*`, `ENABLE_PROMPT_CACHING_1H`.

Terminal and Windows: `CLAUDE_CODE_DISABLE_MOUSE`, `CLAUDE_CODE_DISABLE_MOUSE_CLICKS`, `CLAUDE_CODE_SCROLL_SPEED`,
`CLAUDE_CODE_DISABLE_VIRTUAL_SCROLL`, `CLAUDE_CODE_ALT_SCREEN_FULL_REPAINT`, `CLAUDE_CODE_DISABLE_TERMINAL_TITLE`,
`CLAUDE_CODE_NATIVE_CURSOR`, `CLAUDE_CODE_ACCESSIBILITY`, `CLAUDE_AX_SCREEN_READER`, `CLAUDE_CODE_SYNTAX_HIGHLIGHT`,
`CLAUDE_CODE_FORCE_SYNC_OUTPUT`, `CLAUDE_CODE_FORCE_STRIKETHROUGH`, `FORCE_HYPERLINK`, `CLAUDE_CODE_BS_AS_CTRL_BACKSPACE`,
`CLAUDE_CODE_GIT_BASH_PATH`, `CLAUDE_CODE_USE_POWERSHELL_TOOL`, `CLAUDE_CODE_POWERSHELL_RESPECT_EXECUTION_POLICY`,
`CLAUDE_CODE_DISABLE_WINDOWS_SHELL_LAUNCHER`, `CLAUDE_CODE_SHELL`, `CLAUDE_CODE_TMPDIR`, `USE_BUILTIN_RIPGREP`.

Network: `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `CLAUDE_CODE_CERT_STORE`, `CLAUDE_CODE_CLIENT_CERT`, `CLAUDE_CODE_CLIENT_KEY`,
`CLAUDE_CODE_GZIP_REQUEST_BODIES`, `CLAUDE_CODE_GATEWAY_HINT_HEADERS`, `CLAUDE_CODE_PROPAGATE_TRACEPARENT`.

Debugging: `DEBUG`, `CLAUDE_CODE_DEBUG_LOGS_DIR`, `CLAUDE_CODE_DEBUG_LOG_LEVEL`.

## Not useful for bro

- **Cloud providers**: everything `*_BEDROCK*`, `*_VERTEX*`, `VERTEX_REGION_*`, `*_FOUNDRY*`, `*_MANTLE*`,
  `ANTHROPIC_AWS_*`, `AWS_BEARER_TOKEN_BEDROCK`, `CLAUDE_CODE_USE_ANTHROPIC_AWS`, `CLAUDE_CODE_SKIP_*_AUTH`,
  `CLAUDE_CODE_SKIP_MODEL_ACCESS_MEMORY`, `CLAUDE_CODE_DISABLE_MODEL_ACCESS_FALLBACK`, `CLAUDE_CODE_DISABLE_AUTH_REFRESH_LOCK`,
  workload identity federation (`ANTHROPIC_FEDERATION_RULE_ID`, `ANTHROPIC_ORGANIZATION_ID`, `ANTHROPIC_WORKSPACE_ID`, `ANTHROPIC_PROFILE`).
- **Model aliasing details**: `ANTHROPIC_DEFAULT_*_MODEL_{NAME,DESCRIPTION,SUPPORTED_CAPABILITIES}`, `ANTHROPIC_CUSTOM_MODEL_OPTION*`,
  `ANTHROPIC_DEFAULT_FABLE_MODEL`, `ANTHROPIC_DEFAULT_MODEL`. bro pins model slots itself.
- **Set by Claude Code, read-only**: `CLAUDECODE`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_PID`, `CLAUDE_EFFORT`,
  `CLAUDE_JOB_DIR`, `CLAUDE_CODE_BRIDGE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`,
  `CLAUDE_CODE_REMOTE`, `CLAUDE_CODE_REMOTE_SESSION_ID`.
- **Headless, SDK and CI only**: `CLAUDE_AGENT_SDK_*`, `CLAUDE_CODE_EXIT_AFTER_STOP_DELAY`, `CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS`,
  `CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`, `CLAUDE_CODE_STARTUP_FAILURE_RESULTS`, `CLAUDE_CODE_RESUME_INTERRUPTED_TURN*`,
  `CLAUDE_CODE_RESUME_PROMPT`, `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`, `CLAUDE_CODE_SYNC_PLUGIN_INSTALL*`, `CLAUDE_CODE_SYNC_SKILLS*`,
  `CLAUDE_CODE_TRANSCRIPT_LOCAL_GC`, `CLAUDE_CODE_BG_TASKS_REPORT_RUNNING`, `CLAUDE_CODE_ENABLE_BACKGROUND_PLUGIN_REFRESH`,
  `CLAUDE_CODE_MCP_STARTUP_WAIT_MS`, `MAX_STRUCTURED_OUTPUT_RETRIES`, `CLAUDE_CODE_TEAM_TEARDOWN_PARK_TIMEOUT_MS`,
  `CLAUDE_CODE_USER_DIALOG_TIMEOUT_MS`, `CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST`, `CCR_FORCE_BUNDLE`.
- **OpenTelemetry**: `CLAUDE_CODE_ENABLE_TELEMETRY`, `OTEL_*`, `CLAUDE_CODE_OTEL_*`, `BETA_TRACING_ENDPOINT`,
  `ENABLE_BETA_TRACING_DETAILED`, `CLAUDE_CODE_ENABLE_FEEDBACK_SURVEY_FOR_OTEL`. Only useful with your own collector.
- **Enterprise / managed**: `CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION`, `CLAUDE_CODE_DISABLE_POLICY_SKILLS`, `CLAUDE_CODE_PROCESS_WRAPPER`,
  `CLAUDE_CODE_SCRIPT_CAPS`, `DISABLE_{LOGIN,LOGOUT,UPGRADE,DOCTOR,EXTRA_USAGE,FEEDBACK,INSTALL_GITHUB_APP}_COMMAND`,
  `DISABLE_INSTALLATION_CHECKS`, `CLAUDE_CODE_PERFORCE_MODE`, `CLAUDE_CODE_TOOL_MEMORY_*` (Linux only),
  `CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE`, `FORCE_AUTOUPDATE_PLUGINS`, plugin marketplace plumbing (`CLAUDE_CODE_PLUGIN_*`).
- **Auth bro already handles**: `CLAUDE_CODE_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`,
  `CLAUDE_CODE_API_KEY_HELPER_TTL_MS`, `MCP_CLIENT_SECRET`, `MCP_OAUTH_CALLBACK_PORT`.
- **MCP connection tuning**: `MCP_CONNECTION_NONBLOCKING`, `MCP_CONNECT_TIMEOUT_MS`, `MCP_DISCOVERY_CACHE*`, `MCP_*_BATCH_SIZE`,
  `MCP_PROTOCOL_NEGOTIATION`, `MCP_SDK_GENERATION`.
- **Removed or no-op**: `CLAUDE_CODE_CONNECT_TIMEOUT_MS`, `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION`, `CLAUDE_CODE_ENABLE_AUTO_MODE`,
  `CLAUDE_CODE_ENABLE_OPUS_4_7_FAST_MODE`, `CLAUDE_CODE_OPUS_4_6_FAST_MODE_OVERRIDE`, `CLAUDE_CODE_AUTO_BACKGROUND_WORKER_CHECKIN_SECONDS`,
  `CLAUDE_SUBAGENT_BG_SHELL_MAX_MS`, `TASK_MAX_OUTPUT_LENGTH`.
