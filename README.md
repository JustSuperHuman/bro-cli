# bro

Run your preferred coding harness against **any** model — [Claude Code](https://claude.com/claude-code), [omp](https://omp.sh/), [Pi](https://github.com/earendil-works/pi), the Codex CLI, or [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) — with native, OpenAI-compatible, and Anthropic-compatible providers wired up for you.

Pick a provider, pick a model, go.

## Permission mode

New configurations start Claude Code with `--permission-mode auto`. Set
`permissionMode` in `~/.bro/config.json` to `auto`, `manual`, or `bypass`.
`--safe` selects manual mode; the Skip permissions toggle explicitly enables
`--dangerously-skip-permissions`. Existing `dangerouslySkipPermissions`
booleans remain supported when `permissionMode` is unset. Non-Claude harnesses
retain their own permission flags; Claude auto mode is not passed to them.

## Install

```sh
npm install -g bro-claude
# or: bun install -g bro-claude
```

`bro` is the only package you install up front. Whichever harness you select is
installed globally on first use if its command is missing; the pool's Bun
runtime and the OpenAI-to-Anthropic proxy are repaired the same way. The
installers and command discovery work on Windows, macOS, and Linux.

## Use

```sh
bro
```

1. Scroll to a **provider** and press enter. Aggregators that resell everyone else's models sit in their own **Other Providers** group at the bottom — see [Other Providers](#other-providers--relays-and-the-tier-you-buy-from).
2. Scroll to a **model** and press enter. OpenRouter and the relays load their complete live model catalog; move to the model column and type to filter by model name or id. Press **Tab** to flip the **Skip permissions** toggle (`--dangerously-skip-permissions`) on/off right there, and **h** to rotate the harness (Claude Code · omp · Pi · Codex · DeepSeek).
3. On a relay, pick the **tier** — which upstream route serves that model, and at what price.
4. First time on a paid provider it asks for an API key and saves it.

Every model row shows how the model compares, in columns that adapt to the terminal width:

```
model                          age  cost  $/M in·out   speed  tok/s  quality
Anthropic: Claude Sonnet 5     2mo  $$$$· $2·$10       ●●●○○ 57t/s   ★★★★★ 72
DeepSeek: DeepSeek V3         1.7y  $$··· $0.26·$1.03  ●●○○○ 32t/s   ★★★☆☆ 45
```

| Column | Meaning | Source |
| --- | --- | --- |
| **age** | Time since the model was published (`new` in its first week). | OpenRouter catalog |
| **cost** | `$`–`$$$$$` on a 3:1 input-weighted blend of the per-million-token prices shown beside it (`·····` = free). | OpenRouter catalog |
| **speed** | `●`–`●●●●●` from the median tokens/second across the model's hosts over the last 30 minutes. | OpenRouter endpoint stats |
| **quality** | `★`–`★★★★★` relative to the best coding score in the list; the number is Artificial Analysis' coding index (intelligence index when there is no coding score). | OpenRouter catalog (`benchmarks`) |

A blank cell means nothing is known, not a low score. Narrow terminals drop the numbers first, then the cost and speed columns.

Everything updates from OpenRouter automatically. The catalog is refreshed when it is more than a few hours old (a copy fetched in the meantime is used at once). Speed is only reported to authenticated callers, so it appears once an OpenRouter key is saved or `OPENROUTER_API_KEY` is set: bro measures the newest models in the background while the picker is open, repainting rows as numbers arrive, and keeps each measurement for a day. Other providers' models (Z.ai's GLM, DeepSeek, OpenAI…) are matched to the same catalog by model id, so they get age and quality too; cost is hidden for your own Claude login and local models, where the list price is not what you pay. `bro update` refreshes the catalog and re-measures every model.

Your last provider + model are remembered and pre-selected next time (per
provider). The harness is saved the moment you choose it, so it remains selected
even if a later login, install, or launch fails.

### Terminal Companion integration

When an interactive Claude Code or Codex launch runs inside Terminal Companion's
WindowsTerminalDev/terminal-web bridge, `bro` identifies the foreground agent
automatically. The phone can then show that terminal as Claude or Codex, alert
when it needs input, and render terminal questions as one-tap options. No bro
setting or special launch command is required; account selection, resumed and
forked sessions, the pool, and Claude-on-Codex routes use the same lifecycle.

The handshake is a TTY-only terminal OSC frame containing exactly the protocol
version, agent (`claude` or `codex`), and `active`/`inactive` state. It never
contains a profile, path, command line, environment value, session id, or
credential, and it is silent for headless/piped runs so bro's stdout contract
stays unchanged. This enables Terminal Assist for an existing TUI; it does not
claim that an ACP client has taken ownership of that conversation.

## Headless — one answer, on stdout

`--print` runs Claude Code non-interactively against any provider: it answers
once and exits. **stdout carries the model's answer and nothing else** — bro's
own "Launching…" progress goes to stderr — so it pipes and substitutes like any
other command:

```sh
bro -p zai -m glm-5.3 --print "Summarise src/launch.js in one sentence"
bro -p anthropic --print "Explain this diff" < changes.patch
echo "What does this error mean?" | bro -p zai -m glm-5.3 --print

review=$(bro -p zai -m glm-5.3 --print "Review the staged changes")
bro -p zai -m glm-5.3 --print "List 3 risks" --output-format json | jq -r .result
```

A prompt and piped stdin combine: the pipe is the material, the prompt is the
instruction. A prompt that *starts* with `-` has to be piped — Claude Code reads
a leading dash as a flag no matter how it is quoted, and bro says so rather than
letting it fail obscurely.

Anything after `--print` is handed to Claude Code untouched, so its own headless
flags work as documented — `--output-format json|stream-json`, `--verbose`,
`--resume <id>`, `--allowed-tools`, `--max-turns`:

```sh
bro -p zai -m glm-5.3 --print "Fix the failing test" --output-format stream-json --verbose
bro -p zai -m glm-5.3 --print "…" --allowed-tools "Read,Grep,Edit" --max-turns 8
```

`bro -p` was `--provider` long before Claude Code's `-p` meant print, which is
why the flag is spelled out. The old way still works if you prefer it:
`bro -p zai -m glm-5.3 -- -p "prompt"`.

Headless runs are built for scripts and CI:

| | |
|---|---|
| **stdout is clean** | only the harness writes to it; bro's progress goes to stderr |
| **never prompts** | no provider menu, no model menu, no API-key prompt — it fails with a message and exit 1 instead of hanging |
| **exit codes mean something** | `0` on success, `1` when bro cannot proceed |
| **defaults are left alone** | a scripted run does not overwrite the provider/model/harness your interactive `bro` reopens on |
| **no windows appear** | the [shared browser](#one-shared-browser-for-every-model) is still attached if it is already running, but a headless run never opens one |
| **`-m` is optional** | without it bro takes the first model in the provider's list — the row the menu would have started on |

A missing key is a hard error rather than a prompt, so set it once
interactively (or via the provider's env var, e.g. `ZAI_API_KEY`) before
scripting against it.

Every other route works headless too — the account pool, a named login, and the
Codex bridge all reach Claude Code the same way:

```sh
bro -p pool --print "Summarise today's commits"          # across pooled plans
bro --account <name> --print "Draft the release notes"   # one logged-in profile
bro -p codex --print "Explain this stack trace"          # ChatGPT subscription
```

## Multiple Claude Account Proxy

The **top** option in the menu (`bro -p pool`) pools any number of Claude Max / Team logins behind one local endpoint and launches Claude Code, omp, Pi, or DeepSeek Harness across all of them — so a single session draws from several plans and **fails over automatically** the moment one runs out of usage.

Pick it and `bro` handles everything:

1. **Setup** — if you have no pooled accounts yet, it offers to log in a new one (opens Claude to sign in) or import the login already on this machine. Add as many as you like; each is stored in its own isolated config dir under `~/.claude-max-pool/`.
2. **Start the proxy** — launches the pool server (in `pool/`, runs on [Bun](https://bun.sh)) in the background and waits for it to go healthy. A live dashboard shows each account's auth state, plan, rate tier, and rolling usage at `http://127.0.0.1:3456/`.
3. **Launch the harness** — points Claude Code, omp, Pi, or DeepSeek Harness at the pool. The pool forwards Anthropic `/v1/messages` calls directly to Anthropic with the least-loaded account's OAuth token by default. When the harness exits, the proxy is stopped.

Manage pool accounts directly through `bro`:

```sh
bro accounts login work       # add/log in a new pooled Claude account
bro accounts import primary   # copy this machine's current Claude login
bro accounts list             # show account status and usage
bro accounts remove work      # delete a pooled account
```

See rolling total token usage for this machine's login plus every managed
Claude and Codex profile in one table:

```sh
bro profiles
```

The report shows rolling **24-hour**, **7-day**, and **30-day** totals to the
minute. It runs the pinned `ccusage` CLI in offline mode for each profile, so
Claude streaming records and Codex token deltas, forks, and subagent replays
use its maintained accounting logic. Totals include input, cached, and output
tokens; no account API request is made.

Report lifetime and last-30-day tokens for the local `~/.claude` account, every
account-switcher profile, and every Codex profile, banded by provider with a
subtotal each and one grand total:

```sh
bro tokens
```

```
┌──────────┬─────────────┬────────────────┬────────────────┬─────────────────┐
│ Provider │ Profile     │       Lifetime │       Last 30d │ 30d share       │
├──────────┼─────────────┼────────────────┼────────────────┼─────────────────┤
│ Claude   │ local       │ 78,683,523,672 │ 19,094,386,598 │ ██████▋░░░  66% │
│          │ claude-1    │              — │              — │                 │
│          │ claude-4    │  4,739,383,426 │  4,248,274,580 │ █▌░░░░░░░░  15% │
├──────────┼─────────────┼────────────────┼────────────────┼─────────────────┤
│ Claude   │ subtotal    │ 87,047,077,041 │ 24,199,450,626 │ ████████▍░  84% │
├──────────┼─────────────┼────────────────┼────────────────┼─────────────────┤
│ Codex    │ local       │  8,134,045,086 │  4,758,157,576 │ █▋░░░░░░░░  16% │
├──────────┼─────────────┼────────────────┼────────────────┼─────────────────┤
│ All      │ TOTAL       │ 95,181,122,127 │ 28,957,608,202 │ ██████████ 100% │
└──────────┴─────────────┴────────────────┴────────────────┴─────────────────┘
```

Each provider is read through whichever counter it actually keeps, and the
report says which under the table rather than blending them silently:

* **Claude** comes from each profile's `stats-cache.json` — the same file the
  `/stats` screen renders from, with no pseudo-terminal and no API turn.
  *Lifetime* is Claude's all-time per-model counter, which keeps counting after
  older days roll off, and is the one figure `/stats` never shows. *Last 30d* is
  summed from the daily rows Claude retains and matches the default range on
  `/stats`.
* **Codex** keeps no lifetime counter of its own, so both columns are
  reconstructed from its retained session logs by the pinned `ccusage` CLI in
  offline mode. That is a floor rather than a ledger, and it is labelled as one.

Totals include input, output, and cache tokens. Profiles with no stats cache, or
whose cache stopped updating before the window opened, are listed under **Notes**
with the reason, so an empty row is never mistaken for an unused account.

Note that `bro tokens` and `bro profiles` will not agree on Claude: the former
reports Claude's own counters, while the latter re-derives usage from retained
session transcripts, which are pruned and deduplicated.

Both reports read the local account from `~/.claude` and the local Codex home
from `~/.codex`, deliberately ignoring any `CLAUDE_CONFIG_DIR` / `CODEX_HOME`
inherited from the surrounding shell — otherwise running either command from
inside a pooled session would report that pool profile as "local" and drop the
real local account from the table.

Use one logged-in account directly without the pool:

```sh
bro account                   # pick a Claude account profile from a menu
bro account work              # launch Claude using the "work" profile
bro --account personal        # same shortcut, explicit flag form
```

Profiles are the same standard Claude Code logins stored under
`~/.claude-max-pool/accounts/<name>/`; `bro` switches by setting
`CLAUDE_CONFIG_DIR` for that Claude launch and does not overwrite `~/.claude`.
The interactive profile menu shows each account's current five-hour, weekly,
and Fable usage before you choose one. Beside the logo at the top of `bro`'s
menus, a **Usage** section shows what's left across all your accounts: one
line per app, with its five-hour window and its week in columns — and, on
Claude's line, the same two for Fable:

```
╭─ Usage ─────────────────────────────────╮
│        5h   week              5h   week │
│ ✻     58%    49%    Fable    95%     2% │
│ >_      —    16%                        │
╰─────────────────────────────────────────╯
```

Each figure is that app's own 100%, shared equally by its accounts that have
the window, so nothing passes 100%. Fable only counts the accounts whose plan
reports a Fable limit — the others have no Fable access — and is capped by each
one's overall limits too. An app without a signed-in account gets no line. The
menu opens at once and the numbers fill in as each account answers.

Claude and Codex appear as their app marks rather than their names, in white
(black on a light background). Terminals that can draw images show the real
marks — Windows Terminal 1.22+, xterm, foot, Konsole and WezTerm through
sixel, iTerm2 and WezTerm through inline images, kitty and Ghostty through the
kitty graphics protocol — and every other terminal shows each app's own
terminal mark in bold: Claude Code's `✻` and Codex CLI's `>_`. `bro` asks the
terminal what it supports once, before the first menu; set `BRO_ICONS` to
`sixel`, `iterm`, `kitty` or `text` to choose yourself.
`node scripts/build-icons.js` rebuilds the bundled marks from the installed
Claude and Codex desktop apps.

### One shared browser for every model

On Windows, `bro` gives every Claude Code session the Edge profile you already
use through [hangwin/mcp-chrome](https://github.com/hangwin/mcp-chrome). It uses
the extension's loopback Streamable HTTP endpoint at
`http://127.0.0.1:12306/mcp`: no remote-debugging flag, ports 9222/9223, second
browser profile, or Claude-account-specific browser bridge.

```sh
bro browser setup             # give sessions your own browser (Edge by default)
bro browser setup chrome      # …or name one
bro browser use edge          # pin which browser sessions drive, for good
bro browser open              # open the shared browser
bro browser test              # run an MCP handshake and safely list tabs
bro browser status            # show extension + browser + bridge readiness
bro browser clean             # delete unused bro browser data from earlier setups
bro browser disable           # stop connecting sessions to the browser
bro browser setup edge --claude-extension # opt into Anthropic's older integration
```

Install the unpacked mcp-chrome extension in Edge once. `bro browser setup`
then installs the pinned `mcp-chrome-bridge`, registers its native host for the
current Windows user and Edge, writes this exact config plus the `bro-browser`
skill into every Claude profile, and tests `get_windows_and_tabs`:

```json
{
  "mcpServers": {
    "streamable-mcp-server": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:12306/mcp"
    }
  }
}
```

Bro starts Claude with `--no-chrome`, loads the profile-scoped MCP config, and
sets `CLAUDE_CODE_ENABLE_CFC=false`, so Claude's built-in Chrome integration
cannot compete. mcp-chrome 1.0.31 has an upstream multi-client singleton bug;
Bro applies the reviewed per-transport factory fix from upstream PR #354 until
that fix ships in a release, then verifies the actual browser tool rather than
trusting a port check.

The browser is also the `[b]` switch in the main menu — `OFF · AUTO · EDGE ·
CHROME …`, listing browsers where mcp-chrome is installed. It persists, so it is
a decision you make once. Pass `--no-chrome` to bypass browser tools for one
session.

The bridge is the extension's native host, so it only exists while the browser
is running. Every interactive launch and every `bro browser` command therefore
makes sure it is: when the loopback endpoint is down, bro starts the chosen
browser with `--no-startup-window` (no window appears; the extension connects
the bridge by itself within a few seconds) and quietly proceeds either way.
Use `bro browser status` for the quick loopback check and `bro browser test` for
the complete MCP handshake, tool listing, and safe tab-list call. The native
host's own log lives in `%LOCALAPPDATA%\mcp-chrome-bridge\logs`.

**Why another Claude account gets it too.** mcp-chrome is local and tied to the
signed-in browser profile, not a Claude account. Bro writes the same endpoint
into the local Claude home and every managed account profile. The tools arrive
under `mcp__streamable-mcp-server__*` in each one.

**Which browser.** Keep one mcp-chrome extension connected per port. If both
Edge and Chrome run the extension on 12306, whichever native host binds first
wins. `bro browser use edge` remembers which browser Bro opens, but it cannot
override another already-running extension that owns that fixed port.

The older `bro browser setup edge --claude-extension` route remains available
for Anthropic's account-scoped browser integration. Its optional `--dedicated`
mode maintains a separate Edge user-data directory
(`~/.bro/claude-browser/edge-shared/`) with its own one-time `claude.ai`
sign-in, loads an unmodified-in-store copy of the Claude extension that
prefers Claude Code's native host over Claude Desktop's, and moves that
browser plus its sessions into a private named-pipe namespace via a small Bun
preload so it never competes with the stock bridge. The signed Claude Code
executable is never patched.

### Resuming a session

Under the profiles in that same menu are the sessions you can pick up again —
this project's first, then every other project's with its path. Start typing to
search all of them at once by prompt, project path, git branch, profile, or
session id.

After you choose a session, `bro` asks which login should resume it and
preselects its owner (`local` is this machine's own Claude login; the rest are
pool profiles). Choosing another profile creates a fork there, leaving the
original profile's session untouched. A session from another project still
runs Claude in *that* project's directory.

Session history is read from `~/.claude/projects/` and each profile's own
`projects/` directory, and cached in `~/.bro/sessions.cache.json` — the first
scan takes a moment, later ones are instant.

**Failover:** when the serving account's usage/rate limit runs out before any output has streamed, the pool transparently sidelines it and retries the turn on the next account — you just keep going. Set `CLAUDE_POOL_BACKEND=cli` to use the older subprocess backend. Requires Bun (`bro` finds it automatically; install from [bun.sh](https://bun.sh)). See [`pool/README.md`](./pool/README.md) for the pool's own docs, endpoints, and configuration.

## Codex (ChatGPT subscription)

`bro -p codex` (pinned in the menu) runs your selected harness on your **ChatGPT subscription** — the GPT‑5.x Codex models driving Claude Code, omp, Pi, or DeepSeek Harness through a local bridge. The Codex harness runs its own CLI directly. No API key is needed: just your ChatGPT login.

How it works:

1. **Login** — a built-in ChatGPT OAuth sign-in (the same flow the Codex CLI uses) opens in your browser and stores credentials at `~/.bro/codex-auth.json`. If you already have the Codex CLI logged in, that login is reused automatically. Tokens are refreshed on their own as they expire. Several ChatGPT accounts? See [Codex profiles](#codex-profiles).
2. **Models** — the list is fetched live from your subscription, so it always matches what you can actually run (GPT‑5.6‑Sol, GPT‑5.5, Codex‑Spark, …). Falls back to a cache, then a small built-in list, when offline.
3. **Bridge** — `bro` starts a tiny local Anthropic-compatible server that translates the harness's `/v1/messages` calls into OpenAI Responses-API calls against the ChatGPT Codex backend, and streams the answers back (tool calls, thinking, and usage all mapped through). It's pure Node — nothing to install.
4. **Launch** — Claude Code, omp, Pi, or DeepSeek Harness runs pointed at the bridge. Pick a model in the usual menu (Tab toggles skip-permissions where the harness supports it); `-m <model>` skips it. When the harness exits, the bridge is torn down.

```sh
bro -p codex              # pick a GPT-5.x model, launch Claude Code on it
bro -p codex -m gpt-5.5   # skip the menu
bro -p codex --omp        # use the omp harness instead of Claude Code
bro -p codex --pi         # use Pi through the same subscription bridge
bro -p codex --dsh        # use DeepSeek Harness through the bridge
bro -p codex --codex      # run the codex CLI itself (no bridge, no model menu)
bro codex status          # show login + plan
```

Add `:effort` to a model to set reasoning depth, e.g. `bro -p codex -m gpt-5.6-sol:high`. This impersonates a Codex client to a subscription backend, which is outside OpenAI's normal API terms — use it on your own account at your own discretion.

### Codex profiles

Codex switches logins the same way Claude does, because it keeps its state the
same way: everything for one login — credentials, sessions, settings, history —
lives in one directory, `CODEX_HOME`. So a **Codex profile** is another such
directory under `~/.bro/codex-profiles/<name>`, and `bro` switches by pointing
`CODEX_HOME` at it for that launch. Your own `~/.codex` is never written to.

A new profile starts as a copy of your codex settings (`config.toml`,
`AGENTS.md`, prompts, skills, hooks) so it behaves like the codex you already
configured, then goes its own way with its own ChatGPT login and its own
sessions. The sign-in is `bro`'s own OAuth flow writing that profile's
`auth.json` — the same file the `codex` CLI reads when it runs there, so one
sign-in serves both the CLI and the bridge.

```sh
bro codex                 # pick a profile or session (like bro account)
bro codex work            # run codex under the "work" profile
bro codex profiles        # list profiles and where they live
bro codex login work      # sign a new/existing profile in
bro codex import primary  # copy this machine's Codex login into a profile
bro codex status work     # login state and plan for one profile
bro codex logout work     # drop that profile's credentials
bro codex remove work     # delete the profile, sessions and all
bro -p codex --account work --codex   # launch a profile straight from the menus
```

Profiles show up in the Codex row's right-hand column with their plan and live
five-hour and weekly usage, exactly like Claude accounts show theirs — this
machine's login first, then each profile, then the sessions they can resume.
What's left across all of them is the Codex line of the Usage section. A plan
without a five-hour window (Pro currently reports only the weekly one) shows
`—` there, and a profile imported from another login counts once.

### Resuming a Codex session

Under those profiles are the Codex sessions you can pick up again — this
project's first, then every other project's with its path. Start typing to
search all of them at once by prompt, project path, git branch, profile, or
session id.

After you choose one, `bro` asks which login should resume it and preselects its
owner. Choosing another profile stages a copy there and runs `codex fork`, so
the original login's conversation is left exactly as it was and the continuation
becomes a new session under the profile you picked. A session from another
project still runs codex in *that* project's directory. This happens whichever
harness the toggle is showing: a Codex rollout is a codex conversation, and only
the codex CLI can read it back.

```sh
bro codex resume          # the same list on its own
bro codex resume <id>     # straight back into one you already know
```

Sessions are read from `~/.codex/sessions/` (or `$CODEX_HOME`) plus each
profile's own `sessions/`, and cached in `~/.bro/codex-sessions.cache.json` —
the first scan takes a moment, later ones are instant. Sub-agent threads and
`codex exec` runs are left out: codex's own picker hides them too, and neither
is a conversation you can pick up.

## Harnesses

The `[h]` switch under both menus rotates through the coding agent `bro`
launches, and the choice sticks until you change it:

| Harness | What runs | Works with |
| --- | --- | --- |
| `CLAUDE` | Claude Code (default) | every provider |
| `OMP` | [omp](https://omp.sh/), which picks its own model | every provider |
| `PI` | [Pi](https://github.com/earendil-works/pi), with bro's selected provider/model | every model provider and subscription bridge |
| `CODEX` | the `codex` CLI | your ChatGPT login, OpenRouter, or a provider serving OpenAI's Responses API |
| `DEEPSEEK` | DeepSeek Harness Web UI (`dsh web`) | every API provider, the account pool, and the Codex subscription bridge |

The `[r]` switch is separate: it puts
[Jev Router](#jev-router--a-model-per-turn) in front of `CLAUDE` or `CODEX` so
the model is chosen per turn rather than per session.

```sh
bro --claude              # force Claude Code for this launch
bro --omp                 # force omp
bro --pi                  # force Pi
bro --codex               # force the codex CLI
bro --dsh                 # force DeepSeek Harness Web
bro --harness dsh         # same, long form (claude | omp | pi | codex | dsh)
```

## Jev Router — a model per turn

[Jev Router](https://github.com/gargpratyush/jev-router) chooses the model for
each turn instead of you choosing one for the session: mechanical work goes to
Haiku (or `gpt-5.6-luna`), hard work to Opus (or `gpt-5.6-sol`). The `[r]`
switch under the menus turns it on, and the choice sticks like the harness does.

```sh
bro --jev                     # this machine's Claude login, routed per turn
bro account work --jev        # any Claude profile — the login is unchanged
bro --codex --jev             # the codex CLI on your ChatGPT login
bro codex work --jev          # a Codex profile, routed per turn
bro --no-jev                  # back to picking one model yourself
```

`jev-claude` and `jev-codex` launch the real CLI behind a loopback proxy, so
nothing else bro arranged changes: the profile's `CLAUDE_CONFIG_DIR` or
`CODEX_HOME`, sessions and `--resume`, permission mode, the shared browser and
every flag after `--` all still apply. Claude Code opens with **Jev Router**
selected in `/model`, and Codex with the `jev-router` provider — picking a
concrete model there pauses routing, picking Jev Router again resumes it. Run
`/jev-explain` (Claude) or `$jev-explain` (Codex) to see why a turn was routed
where it was.

Routing needs a [TypeSafe](https://docs.typesafe.ai) key:

```sh
echo "JEV_API_KEY=..." > ~/.jev-router.env   # PowerShell: Set-Content "$HOME\.jev-router.env" "JEV_API_KEY=..."
```

Without one the CLI still starts, just unrouted — `bro` says so before the
session opens. The `jev-router` package installs itself on first use.

It fronts Claude Code on a Claude login and the `codex` CLI on a ChatGPT login.
Anything that already points a CLI somewhere else — an Anthropic-compatible
provider, the `ccr` proxy, the account pool, the Codex bridge, and the omp / Pi
/ DeepSeek harnesses — says why it can't be routed and runs unrouted rather
than failing the launch.

### DeepSeek Harness provider compatibility

DeepSeek Harness already owns a first-class provider/model switcher, so bro
does not replace it with a single locked route. For each launch, bro supplies a
temporary DSH composition overlay containing every merged bro API provider and
all of its models. API routes are namespaced as `bro-<provider>` and labelled
`Bro · <Provider>`. Claude uses DSH's `anthropic` catalog route so current
thinking modes, context windows, modalities, and token limits are inherited.
DSH's installed catalog, providers added in its Models page, credentials,
settings, sessions, and plugins remain untouched. OpenRouter is hydrated from
its complete live catalog before the overlay is built.

The provider/model selected in bro becomes DSH's composition default for a
fresh profile. A model previously selected inside DSH deliberately wins over
that default, and you can switch providers or models normally in the Web UI.
Keys from `~/.bro/config.json` are supplied through per-process environment
references and are never written into the temporary overlay or command line.
DSH's own credential store and user settings can override any bro route.

Every DeepSeek launch through `bro` also autoloads a **Profiles** action in
DSH's sidebar. It combines this machine's Claude Code login, every Claude
account under `~/.claude-max-pool/accounts/`, this machine's Codex login, and
every Codex profile under `~/.bro/codex-profiles/`. Claude rows show the live
five-hour, weekly, and Fable meters; Codex rows show its primary and secondary
rate-limit windows. Logged-out profiles remain visible but disabled, and one
failed usage request or bridge does not hide the other accounts.

Choose a row to change the active DSH session's real provider route. The
current model is kept when that profile offers it; otherwise the route's
default model is selected. DSH's built-in model picker sees the same change,
so the next request actually uses that login. Account credentials remain in
their Claude/Codex stores behind launch-scoped loopback bridges. The plugin is
linked from the installed `bro` package and is refreshed automatically with
`bro`; after adding or logging into a new profile, restart DSH once so its
provider route can be created.

`--safe` maps to DSH's `workspace-write` permission mode; the normal
skip-permissions setting maps to `danger-full-access`. Arguments after bro's
flags go to the Web app, for example `bro -p deepseek --dsh --port 4080`. Bro
opens the exact readiness URL DSH prints, then keeps DSH in the foreground. Set
`BRO_DSH_NO_OPEN=1` when a supervisor should own the browser lifecycle.

DeepSeek Harness is currently a developer preview and requires Node.js 22.19.x
or 24+. A missing `dsh` command installs `@deepseek-ai/dsh@latest` on first use.
Because the preview moves quickly, it also has an explicit update path:

```sh
bro harness install dsh   # optional eager install
bro harness update dsh    # reinstall the current npm latest tag
bro update dsh            # short alias for the same update
```

Selecting Claude automatically connects the active Claude Code OAuth login
(`$CLAUDE_CONFIG_DIR` or `~/.claude`) through a private, refresh-safe loopback
Anthropic bridge. Token refreshes are persisted back to the real Claude login,
the accessible Claude model list is loaded live, and the real OAuth token never
enters DSH's settings, patch, command line, or environment. The bridge exists
only while DSH is running. If Claude Code is logged out, run `claude` and finish
`/login`; `ANTHROPIC_API_KEY` remains a supported fallback. The multi-account
pool still provides account rotation/failover, while Codex subscription access
uses bro's Codex bridge. Claude CLI transcripts remain CLI-specific; DSH keeps
its own resumable sessions in its normal harness home.

For custom/API providers, `bro` upserts only its namespaced provider entry in
`~/.pi/agent/models.json` (or `$PI_CODING_AGENT_DIR/models.json`); existing Pi
settings and providers stay intact. The real API key is supplied in a
launch-only environment variable, never written to that file or exposed in
command arguments. Pi has no permission popups, so
the skip-permissions toggle does not add a Pi flag. On native Windows, Pi itself
requires a Bash shell; Git for Windows satisfies that upstream requirement.

If a harness command is missing, first use installs the official package:
`@anthropic-ai/claude-code`, `@oh-my-pi/pi-coding-agent`,
`@earendil-works/pi-coding-agent`, `@openai/codex`, or
`@deepseek-ai/dsh@latest`. npm is used where
available, with Bun as the supported fallback; omp uses Bun or its official
platform installer.

Codex is the narrow one: as of codex 0.147 it speaks only OpenAI's Responses
API, so it runs on your ChatGPT subscription or against an OpenAI-format
provider that serves `/responses` — `bro` writes that provider into codex's
config for the one run (`-c model_providers.…`, key passed by environment
variable) and never touches `~/.codex/config.toml`. Anthropic-shaped providers
— native Claude, the account pool, OpenRouter, Z.ai — have no route into codex
and are refused up front rather than failing mid-turn. Skip-permissions maps to
codex's `--dangerously-bypass-approvals-and-sandbox`.

## ✦ JustImagine — images & video

`bro imagine` (also the first option in the menu) doesn't launch a harness at all. It asks which image API to use, then serves a local gallery and opens it in your browser. Everything runs on your machine; nothing leaves it except the call to the generation API.

```sh
bro imagine                    # pick an API, then the gallery opens
bro imagine -p openrouter      # skip the API menu
bro imagine --root D:/Art      # keep the gallery somewhere else
bro imagine service install    # run it in the background, at every login
bro imagine skill              # teach an agent to drive it (generate-images-videos)
```

- **Images** — any OpenAI-shaped `/images/generations` API, plus the chat-routed image models (Gemini / Nano Banana, GPT-5 Image) that aggregators serve through `/chat/completions`. Size and quality knobs where the API supports them.
- **Video** — OpenRouter's video API: Veo 3.1, Sora 2 Pro, Seedance 2.x, Wan 3.0, Kling v3, Hailuo 3, Runway Gen-4.5, Grok Imagine and the rest of the catalogue, refreshed live at startup (a bundled snapshot keeps it working offline). Each model's own duration, resolution, aspect-ratio, audio and seed options drive the controls, so you can only ask for a combination that model actually accepts. Attach a reference image and a model with first-frame conditioning animates it.
- **Model picker** — two panes: a list you scan on rank and price, and a detail column for whatever is under the cursor or the keyboard, so you can tell what a model is *for* before you spend anything on it. The detail side carries the publisher's own description of the model, the capabilities read straight off the catalogue (clip length, top resolution, audio, first/last-frame conditioning, seed, whether it takes your reference images), and every number behind the meters spelled out. Search to filter; sort by **Recommended**, **Best**, **Cheapest** or **Newest**; arrow keys and Enter to pick.

  | Fact | Image models | Video models |
  | --- | --- | --- |
  | **age** | time since the model was published (`new` in its first week) | same |
  | **cost** | estimated price per picture, from OpenRouter's output-token price and the family's tokens per image | OpenRouter's list price per second of video at 720p (or the plain rate), with a 5-second estimate on hover |
  | **speed** | how long that model has actually taken in this gallery (median of your own generations) | same |
  | **quality** | Design Arena rank in the image category, out of every model on the board | Design Arena rank in the video category |

  Only OpenRouter publishes these facts, so a model served under a shorter id by an aggregator or a first-party API is **matched to the same model in the catalogue** and shown its figures — which is what turns a bare list of ids into something comparable. A borrowed price says whose price it is, because another shop may well charge differently. The first-party Images API models (DALL·E 3, GPT Image 1) are in no catalogue at all, so their published per-image prices are carried in `MODEL_PRICING` in `src/justimagine-gen.js`, each with the size and quality it assumes; a model entry in your own `imageApis` config can state its own `pricing` and override it.

  Quality comes from [Design Arena](https://www.designarena.ai)'s public leaderboard directly, not from OpenRouter's embedded snapshot of it. The snapshot ranks a model only among the ones OpenRouter serves — so two different models could each show "#2" in the same list — and it covers neither video nor the first-party Images API models. Going to the board itself gives one rank scale across the whole picker, ranks video (which had no rating at all), and ranks DALL·E 3, GPT Image 1 and GPT Image 2. A rank is shown with the size of the field and the number of head-to-head votes behind it. Names are matched allowing for spelling (`wan-v3.0-t2v` is `alibaba/wan-3.0`) but never across versions or tiers — `veo-3` and `veo-3.1` stay separate, because putting another model's score on a row is worse than showing none.

  Where a price is genuinely unquotable the picker says which — a router costs whatever it routes to, an upscaler is billed per megapixel of the clip you hand it — rather than leaving a dash to look like a failed lookup. Everything refreshes from OpenRouter when the gallery starts; speed fills in as you generate.

  The menu sizes itself to the window: it takes the width available up to 880px, slides back from the edge only as far as it must (a button halfway along a wrapping toolbar has room on neither side, so aligning to either edge would push half the menu — and the price column with it — off screen), opens upward when the room below is too short, stacks its two columns when too narrow for both, and becomes a full-screen sheet on a phone.
- **Settings** — the gear in the header: every provider, whether it is ready, and a field to paste a key into, saved straight to `~/.bro/config.json` and live within seconds without a restart. A key set through an environment variable is shown as the shell's to change rather than offered a Remove that wouldn't stick. Keys are never sent back to the page — only a masked preview like `sk-o…b185` — and a write is refused unless it came from the gallery's own origin, so no other site in your browser can reach the loopback port to spend your credits or overwrite a key.
- **Folders, not batches** — the sidebar is the folder tree of your gallery root, and whatever you generate lands in the folder you have selected. Make folders, nest them, rename them inline, drag generations between them or move a selection with one menu. **Deleting a folder deletes every generation inside it**, including its metadata and cached thumbnails.
- **✨ Improve the prompt** — the button beside the prompt box rewrites a one-line idea into something the picked model can work with, server-side on `gemini-3.7-flash` via the same OpenRouter key. It knows what it's writing for: an image model gets composition and light, a video model gets a named camera move and what changes across the shot (bounded by that model's real clip length), and a character's description gets the permanent look only. Picked characters keep their names and are never re-described. `↺` puts back exactly what you typed. Ctrl+Enter does the same from the keyboard.
- **Repeatable characters** — a cast you define once and reuse everywhere. Give a character a name, a description and a few reference images; pick it in the composer and its pictures ride along with the prompt while it's named in the text, so the same face comes back shot after shot. Paste, drop or browse images straight into a character's reference area — or, with no photos to start from, **Draw 5 reference shots** builds the sheet for you on Nano Banana 2: one portrait from the description, then four more angles drawn *from that portrait*, so they are one character rather than five people matching the same sentence. Pick the ones worth keeping; the rest are discarded. A generation you liked can be promoted into one of its references (`👤` on the card), which is how a character sharpens as you work. The library is global — `~/.bro/justimagine/characters` — so it's there in every gallery and in the background service.
- **Light or dark** — a toggle in the header, defaulting to light whatever your OS prefers, remembered per browser and applied before the first paint so there is no flash.
- **Reference images** — paste, drag-drop or attach. They're saved under `.context/` named by content hash (the same picture is never stored twice) and appear in a strip for one-click reuse.
- **Built to stay quick** — generation is asynchronous on the server and streamed to the page over server-sent events, so a five-minute video survives a reload and no request is held open. The grid loads cached thumbnails, not originals; posters are captured once in the browser and reused forever; a video tile downloads no video bytes until you open it, and then over byte ranges so it can seek. Images load on approach and drop their decode again once well out of view.

```text
<root>/                    folder "" — generations made at the top level
  history.jsonl            metadata for the media beside it
  <name>/                  a folder you made; nests arbitrarily
  .context/                reference images
  .thumbs/                 derived posters (safe to delete)

~/.bro/justimagine/characters/
  nora/character.json      name, description, cover
  nora/refs/<sha>.png      that character's own reference images
```

> Characters are reference-driven, not trained. That's the same mechanism as Higgsfield's avatars and its `nano_banana_2 --image ref.png` path — good, and better the more references you give it — but it is not Higgsfield's *Soul Character*, which trains a model on a face. Nano Banana Pro holds a likeness best for stills; for video, the references go to `input_references` so the character guides the shot without pinning frame one.

Metadata lives per folder rather than in one index, so a folder survives being moved by hand and deleting one leaves nothing dangling. A gallery from the old `bro image` (`./.bro/image-gen`) is folded into the new root the first time you run it.

Keys are shared with the chat provider of the same id, so a saved Yunwu key just works; video always uses the `openrouter` key. Add or replace a key from the gallery's own Settings panel, or edit `keys` in `~/.bro/config.json` by hand — either way both surfaces see it. Add your own APIs via `imageApis` in `~/.bro/config.json` (merged by `id`, same as providers); a model entry there can carry a `description` of its own, which the picker shows.

### Run it as a background service

One switch turns JustImagine into a service that comes back on its own — **no admin or root required**:

```sh
bro imagine service install [--root <dir>] [--port <n>]
bro imagine service status | start | stop | restart | logs | uninstall
bro imagine open               # open whatever is running
```

It installs **the gallery of the directory you run it in** — `./.bro/justimagine`, the same one `bro imagine` serves there — on port 8791. So `cd` to the project you want served and install; pass `--root` to serve somewhere else. Install again in another directory and it moves, saying which folder it left behind.

**Elevation decides when it starts, not whether it works.**

| | Unelevated | Elevated (`sudo`, or an Administrator terminal) |
| --- | --- | --- |
| Starts | when you log on | with the machine, before anyone logs on |
| Stops | when you log out | never — it is a real system service |
| Runs as | you | still you, so it reads your keys and writes your gallery |

Unelevated is the default and prints a plain warning that it **only runs while you are logged on**, with the command to fix it. Re-run the install elevated and the system-wide one **takes priority automatically**: the per-user install is stopped and removed first, because two servers cannot share a port. Going the other way is refused rather than silently downgrading — an unelevated install over a system one changes nothing and tells you so.

| Platform | Per-user (at login) | System (at boot) |
| --- | --- | --- |
| Windows | Task Scheduler task `JustImagine`, logon trigger, registered from XML (`/SC ONLOGON` needs elevation; the same trigger as XML does not) | a second task, `JustImagine-System`, with a boot trigger and an **S4U** principal — which is how a task runs as you with no stored password |
| macOS | launchd LaunchAgent in `~/Library/LaunchAgents` | launchd LaunchDaemon in `/Library/LaunchDaemons` with `UserName` and your `HOME` |
| Linux | systemd `--user` unit (plus `enable-linger`), or an XDG autostart entry where there is no systemd | systemd system unit with `User=` and `HOME=`, `WantedBy=multi-user.target` |

Both Windows scopes launch through a `wscript` shim, so no console window ever appears and the service can be handed the environment Task Scheduler's XML has nowhere to put. Because that shim detaches, the trigger repeats every five minutes as a keepalive: an attempt that finds the port already answering exits quietly, and one that finds it dead brings the gallery back. launchd and systemd supervise their own with `KeepAlive` and `Restart=always`.

Logs land in `~/.bro/justimagine/service.log`, `uninstall` removes whichever scope is installed and leaves your gallery completely untouched.

### JustImagine HTTP API — and a skill for agents

The gallery has no private browser-only path: the page calls JSON routes, and anything else can call the same ones. The routes an agent needs are built for volume — one request queues a whole set, another blocks until it lands:

```sh
BASE=http://127.0.0.1:8791

# 1. what can this machine actually run, and what does it cost?
curl -s "$BASE/api/models?kind=video" | jq '.models[] | {id, ready, pricing, durations}'

# 2. queue a mixed set — different prompts, images and clips together
curl -s "$BASE/api/batch" -H "content-type: application/json" -d '{
  "defaults": { "folder": "Campaign", "model": "google/gemini-3.1-flash-image" },
  "items": [
    "a cyclist at dawn on a wet city street",
    { "prompt": "the same cyclist outside a cafe", "count": 3 },
    { "prompt": "a slow push-in on the cafe window", "kind": "video",
      "model": "google/veo-3.1", "duration": 8, "audio": true }
  ]
}'                                        # → {"jobs":[…5 ids…],"images":4,"videos":1}

# 3. wait for them (or pass "wait": 120 in step 2 and skip this)
curl -s "$BASE/api/jobs?ids=$IDS&wait=60" | jq '{settled, items, failed, pending}'
```

Eight images and three clips run at once, so a batch of fifty paces itself; finished jobs stay readable for half an hour, so a poller that is minutes late still collects everything; a generation that fails upstream comes back in `failed` with its reason rather than as an HTTP error. Reference images can be registered from a path on disk (`{"path":"/photos/bottle.png"}`) instead of base64, and an output you just made is a valid input for the next call. `POST /api/cancel {"all":true}` stops a batch you regret. Full route list, the folder model, characters and reference handling: **[docs/justimagine-api.md](./docs/justimagine-api.md)**.

```sh
bro imagine skill              # → ./.claude/skills/generate-images-videos/SKILL.md
bro imagine skill --global     # → ~/.claude/skills/… (every project)
bro imagine skill --dir <path> # anywhere else
```

`bro imagine skill` installs **`generate-images-videos`**, a skill that hands an agent the whole workflow rather than a route list: find or start the server, read `/api/models` before spending anything, batch instead of looping, poll with `wait=` rather than sleeping, attach references and characters, collect the files off disk — plus the caps (50 items and 100 jobs per batch, 12 images or 4 clips per item), and the etiquette that matters when each call costs real money: price a batch before queueing it, say what failed, and never delete a folder to tidy up. It ships in the package at [`skills/generate-images-videos/`](./skills/generate-images-videos/SKILL.md), so it is also the file to read yourself if you are writing your own client.

## Providers

Claude is next in the list and runs **natively** with the Claude harness (your normal Claude login — no proxy). Other Anthropic-compatible providers (OpenRouter, Z.ai) are passed directly to Claude, omp, Pi, or DeepSeek Harness. OpenRouter also exposes its OpenAI Responses endpoint, so the Codex harness routes OpenRouter models through that endpoint automatically. OpenAI-format providers (Sakana, OpenAI, DeepSeek, Groq, …) use [`claude-code-router`](https://github.com/musistudio/claude-code-router) for Claude, while omp, Pi, and DeepSeek Harness receive native provider entries. `bro` installs any missing helper on first use.

### Other Providers — relays, and the tier you buy from

The last group in the picker is **Other Providers**: aggregators that resell
everybody else's models behind one endpoint. [OpenLux](https://api.openlux.ai)
and [Yunwu](https://yunwu.ai) are there today. They sit in their own section
because they carry the same model *names* as the first-party providers above —
`claude-opus-5` from Anthropic and `claude-opus-5` from a relay are not the same
purchase.

bro browses their full live catalogue (OpenLux is ~270 chat models, Yunwu ~200),
fetched unauthenticated from the relay itself and cached like the OpenRouter
one, so the model column shows everything on offer rather than a handful of
names baked into `models.json`. Type to filter it. Age and quality still come
from the OpenRouter catalog by model id; the price is the relay's own.

The part that matters is the **tier**. A relay serves one model through several
upstream routes — an official API key, an Azure deployment, a subscription
client — and each route is a named group with its own price multiplier. Same
model, same name, order-of-magnitude different bill:

```
tier (upstream route)              price ×  $/M in·out
Codex-Gpt-1                         ×0.037  $0.368/$1.84 per M   (cheapest)
Codex-Gpt-3                         ×0.074  $0.735/$3.68 per M
Azure-Gpt-5                          ×0.441  $4.41/$22.1 per M
Openai-Gpt-2                         ×0.735  $7.35/$36.8 per M
```

So after you pick a model, bro asks which tier — showing only the routes that
actually serve *that* model, cheapest first, priced per million tokens. The
model column itself is priced at each model's cheapest tier, so it answers
"what would this cost me at best". Skip the menu with `--tier`:

```sh
bro -p openlux -m gpt-6-astra --tier Codex-Gpt-1
bro -p openlux -m claude-opus-5 --tier Claude-Code-1
bro -p yunwu  -m claude-opus-4-8
```

bro also routes per model, not per provider: a model the relay serves in
Anthropic format runs Claude Code straight against `/v1/messages`, and everything
else goes through the OpenAI proxy — you don't have to know which is which.

**Tokens are per tier.** The relay fixes a token's group when you create it in
its console; it cannot be set per request. So bro keeps a key per tier —
`openlux@Codex-Gpt-1` in `~/.bro/config.json` — and asks for one the first time
you use a tier you have no token for. A plain `openlux` key (or `OPENLUX_API_KEY`)
stays the fallback, so a single token still launches every tier it is allowed to
reach. A green `•` in the tier menu marks the tiers you already have a token for.

Any new-api gateway works the same way — add one under `providers` in
`~/.bro/config.json` with `"catalogue": "newapi"` and `"section": "other"`, and
bro will browse its catalogue and offer its tiers too.

### Flags

```sh
bro -p pool               # Multiple Claude Account Proxy (pool many plans)
bro account work          # launch Claude using one logged-in account profile
bro browser setup         # give every model bro launches your own browser
bro browser use edge      # pin which browser sessions drive
bro browser status        # show browser/extension/bridge readiness
bro -p codex              # Codex on your ChatGPT subscription (live model list)
bro codex                 # pick a Codex profile or session
bro codex resume          # resume a Codex session
bro --pi                  # launch Pi (also --omp / --codex / --dsh / --claude)
bro --jev                 # Jev Router picks the model each turn (--no-jev off)
bro -p sakana -m fugu     # skip the menus
bro -p openlux -m gpt-6-astra --tier Codex-Gpt-1
                          # relay: pick the upstream route (and its price)
bro -p zai -m glm-5.3 --print "prompt"
                          # headless: one answer on stdout, then exit
bro --list                # list every provider + model
bro update                # refresh the model list from GitHub, cache it locally
bro update dsh            # update DeepSeek Harness to npm latest
bro --dry-run             # show what would run, launch nothing
bro --safe                # don't pass --dangerously-skip-permissions
bro --resume <session-id> # pick provider/model, then resume Claude there
bro -p pool --resume <id> # resume through the Multiple Claude Account Proxy
bro -- --help             # force a bro flag name through to the harness
```

Put `bro`'s own flags first. The first unrecognized argument, and everything
after it, is passed verbatim to the selected harness after provider/model
selection.

`bro`'s own progress is written to stderr, so stdout only ever carries what the
harness printed — see [Headless](#headless--one-answer-on-stdout).

## Config

Keys and your own providers/models live in `~/.bro/config.json`:

```jsonc
{
  "keys": {
    "sakana": "fish_...",
    "openlux@Codex-Gpt-1": "sk-...   ← a relay token is created for one tier",
    "openlux": "sk-...   ← fallback for every other tier",
    "#openai": "sk-...   ← any key starting with # is ignored (notes / test data)"
  },
  "providers": [
    {
      "id": "mylocal",
      "name": "My Local LLM",
      "mode": "openai",
      "baseUrl": "http://localhost:1234/v1/chat/completions",
      "noKey": true,
      "models": [{ "id": "my-model", "name": "My Model" }]
    },
    {
      "id": "myrelay",
      "name": "My new-api Relay",
      "section": "other",
      "catalogue": "newapi",
      "mode": "anthropic",
      "baseUrl": "https://relay.example.com",
      "keyUrl": "https://relay.example.com/console/token",
      "models": [{ "id": "claude-opus-5", "name": "Claude Opus 5" }]
    }
  ]
}
```

`"defaultHarness"` picks the harness a fresh install opens on, and
`"jevRouter": true` turns [Jev Router](#jev-router--a-model-per-turn) on by
default; the `[h]` and `[r]` switches override both and remember your choice.

Custom providers merge with the built-in list (same `id` adds models; new `id` adds a provider). `"section": "other"` files a provider under **Other Providers**; `"catalogue": "newapi"` marks a [new-api](https://github.com/Calcium-Ion/new-api) relay, whose whole catalogue bro fetches from `{baseUrl}/api/pricing` and whose tiers it offers per model — the `models` list is then only an offline fallback. The built-in model list is pulled from [`models.json`](https://github.com/JustSuperHuman/bro-cli/blob/main/models.json) on GitHub and cached at `~/.bro/models.cache.json` — run `bro update` to refresh it (override the source with `BRO_MODELS_URL`).

---

Made by [JustGains](https://justgains.com) · MIT
