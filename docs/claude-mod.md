# The bro mod for Claude Code

A Claude Code mod (a plugin of function hooks) that links Claude sessions to bro. Turn it on under
Settings › bro mod › Load the mod (off by default); Claude sessions launched afterwards load it.

It gives Claude two things:

- **/bro**: every session you have open in bro, most urgent first and a press away from each, plus every
  login's limits, in a pane. `/bro go <session>` brings one to the front, `/bro title <text>` renames this
  one, `/bro limits` lists the limits.
- **questions**: Question chime, Question timer, Answer within (15s to 10 min, 30s by default), Countdown
  ticks.

Saved in `~/.bro/v2.toml`:

```toml
[claude_mod]
on = false     # load the mod into new Claude sessions
chime = true   # a chime when Claude asks you a question
timer = false  # Claude's questions count down, then Claude uses its best judgement
timer_secs = 30 # how long a question waits with the timer on
ticks = true   # the timer ticks for its last 15 seconds
```

## How it works

Source in `crates/bro-core/claude-mod/`, built into bro and written to `~/.bro/claude-mod/bro-<hash>/` (one
folder per build, so a running session keeps the copy it started with). bro passes `--plugin-dir` and the
options as `pluginConfigs.bro.options`. Outside a bro session (no `BRO_SESSION_ID`) every hook passes through.

Three parts:

- **link/**: `/bro` and its pane, which polls bro only while it's open. The orchestrator opens the pane as
  its sidebar when the terminal is wide enough to dock it.
- **questions/**: Claude's questions (AskUserQuestion). bro chimes as one opens. With the timer on, a countdown
  bar runs above the question (Claude Code takes nothing below it) and ticks a second for its last 15
  seconds, firmer for the last five. At zero the mod answers for you: the question closes as Claude Code's
  own away-from-keyboard answer does (`afkTimeoutMs`), with a note telling Claude to go with its best
  judgement and say which way it went. The mod passes the timer as `timer` in seconds, 0 for none.
- **apps/**: MCP Apps for bro's focus mode. When a tool answers with a widget (a `ui://` resource), bro
  draws it in the conversation, but the widget's HTML and the tools it calls sit behind Claude Code's own MCP
  connections (claude.ai connectors and their sign-ins, stdio servers). So bro asks the session's mod, and the
  mod does the work and answers. See below.

The mod draws in Claude Code's own theme colours. The sounds are bro's own, made in `bro_voice::cue` and
played by `bro hook sound question` and `bro hook sound ticks <closes at, epoch ms> <seconds>` on bro's
player, the same on every OS. The mod finds bro through `BRO_BIN`. The ticks come from one player started two
seconds before the first, which times them to the close itself; answering ends that player at once.

The link talks to the bridge of the bro that launched the session (`BRO_BRIDGE_URL`). Without one (the bridge
off, or a second bro opened beside a running one) it stays quiet rather than ask another bro.

### Apps: bro asks, the mod answers

While the session runs, the mod keeps one long poll open on the bridge: `GET /api/mod/<BRO_SESSION_ID>/jobs`.
The bridge answers as soon as bro has work, with every job waiting (`{ jobs: [{ id, kind, ... }] }`), or
with none after 25 seconds. The mod runs each job and posts the answer to `/api/mod/<session>/jobs/<id>`:
`{ ok: true, result }` or `{ ok: false, error }`. When bro is away it asks again five seconds later.

- `{ kind: "read", server, uri }` reads the resource with Claude Code's ReadMcpResourceTool and answers
  `{ contents: [{ uri, mimeType, text }] }`.
- `{ kind: "call", server, tool, arguments }` calls the tool with `$.mcp.call` and answers its MCP result
  (`content`, `isError`, `structuredContent`).

`server` is spelled as in tool names (`claude_ai_JustGains`). Only callers on this machine reach these routes,
token or not. In bro, `Bridge::mod_ask` queues a job and gets the answer, or an error: nobody took it within
10 seconds (the session has no mod listening, so turn the mod on and start a new Claude session), no answer
within 90 seconds, or the bridge stopped. `Bridge::mod_listening` says whether the session's mod asked for work
in the last 40 seconds.

A job never puts a permission prompt in front of you. The jobs run outside the mod's own hooks, so the mod
can't approve its own reads. Before reading, it asks Claude Code what it would decide, which shows nothing,
and reads only when the answer is allow. That is Claude Code's own answer for ReadMcpResourceTool, unless one
of your rules or another plugin says otherwise; then bro gets the reason instead of the widget. Tool calls
through `$.mcp.call` never prompt.

bro also tells every Claude launch `BRO_PROFILE` (the login whose config dir it runs in), `BRO_BACKEND` (who
serves the model: the login, a provider id, `pool`) and `BRO_VERSION`.

Working on the mod:

```sh
claude plugin validate crates/bro-core/claude-mod
claude --plugin-dir crates/bro-core/claude-mod     # with BRO_SESSION_ID and BRO_BRIDGE_URL set
```

Every file the mod holds must be listed in `claude_mod::FILES` (a test checks). Its tests sit in
`crates/bro-core/claude-mod/tests/` (not built in) and run with `claude plugin test crates/bro-core/claude-mod`.
