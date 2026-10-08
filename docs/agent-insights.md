# Agent insights: why it's waiting, recaps, files it touched or sent, llmtrim

This doc covers four features, inspired by the herdr plugins named in brackets:

* **why it's waiting** (herdr-island)
* **recap** (herdr-recap)
* **files the agent touched** (herdr-openr)
* **files it sent you** (herdr-shelf)

All four come from one source: tailing each live agent pane's transcript. There are no
hooks and no changes to the user's Claude or Codex settings. The fifth feature, optional
request compression (llmtrim), lives in bro-proxy.

## 1. Binding a live pane to its transcript (bro-core)

* **Claude.** Read `<CLAUDE_CONFIG_DIR>/sessions/<pid>.json` (`{ sessionId, cwd }`), using the
  pid of the claude process bro spawned. Then use `sessions::find_in_profile(id)` to get
  `projects/<slug>/<id>.jsonl`. Re-check every ~5 s, because `/clear` and `/resume` switch
  the id. If that fails, fall back to `sessions::latest_for(Claude, profile, cwd, pane_started - 5s)`.
* **Codex.** For a resume or fork, use `find_by_id`. Otherwise use
  `latest_for(Codex, profile, cwd, pane_started - 5s)`, re-checked until it is found and
  then every ~15 s.
* **Pi and omp** get nothing for now, and their insights stay empty.

## 2. Tailing (bro-core `sessions::live`)

The reader is incremental. It keeps a byte offset and a partial-line buffer, and resets
if the file shrinks or is replaced. The first read looks at the last 4 MB only. It never
blocks the TUI thread. It produces `AgentFacts`:

* **`sent`** (Claude only). Comes from successful `SendUserFile` calls. Prefer
  `toolUseResult.attachments[]`, which gives `path`, `size`, `isImage` and `media_type`.
  Otherwise use `input.files[]` when it is a real array. Skip `is_error` results. Keep
  the caption and time. Dedupe by path, newest first.
* **`touched`**, newest first and deduped by path. Each entry has a kind: `edit`,
  `write`, `create`, `delete`, `move` or `read`.
  * Claude: Edit, MultiEdit and Write use `file_path`. NotebookEdit uses
    `notebook_path`. Read uses `file_path` plus `offset`, which becomes the line.
    Results with `is_error` are skipped. A Write whose result has `type:"create"`
    is `create`.
  * Codex: `event_msg` items `item_completed` with `type:"FileChange"` and
    `status:"completed"`. Each key of `changes` is a path with `add`, `update` or
    `delete`, and a non-null `move_path` means `move`. Reads come from
    `CommandExecution.parsed_cmd[].type=="read"` and its `path`. The fallback is the
    older stock format: `custom_tool_call name:"apply_patch"` with `*** (Add|Update|Delete) File: <path>`
    lines, or `patch_apply_end`.
* **`pending`**: the last tool call that has no result yet. Its reason text follows
  herdr-island:

  | Tool call | Reason text |
  |---|---|
  | Bash | `Bash: <description \|\| command>` |
  | Edit, Write, Read, NotebookEdit | `Edit: <basename>` (tool name, colon, basename) |
  | AskUserQuestion | `Question: <questions[0].header \|\| question>` |
  | ExitPlanMode | `Approve the plan?` |
  | `mcp__srv__tool` | `MCP: tool` |
  | anything else | the tool name |
  | Codex `request_user_input_async` | `Question: <questions[0].title>` |

  Reasons are truncated to 80 characters with `…`.
* **`asked`**: set when the turn ended on a question (attention-queue's rule). It looks
  at the last ~700 characters of the final assistant text. A line ending in `?`, or
  "let me know / confirm / which / should I / want me to / up to you", counts. Text in
  quotes or code blocks is ignored. The value is that line, trimmed to 80 characters.
* **`away_summary`**: the newest `{"type":"system","subtype":"away_summary","content"}`
  record, with its timestamp.
* **`last_assistant`**: the first sentence or line of the last assistant text, up to
  160 characters, with its timestamp.
* **`last_activity`**: the newest record that has a `timestamp`.

## 3. Attention and recap (bro-tui owns the policy)

* **Status** stays as it is today: screen-based, from `Term::scan`.
* **Why it's waiting** applies when status is *needs you*. The reason is
  `pending.reason`, else the screen prompt title (`input_context().prompt.title`, via
  the bridge's `prompt::detect_prompt` on the pane screen), else `asked`, else none.
  When status is *done*, the reason is `asked` if present. Otherwise there is none.
* **Recap**, first match wins:
  1. `away_summary` when it is newer than `last_assistant` (free; Claude wrote it).
  2. A cached generated recap whose transcript size and mtime are unchanged.
  3. `last_assistant`, labelled source `last-message`.
* **Generated recap**. This is setting `[insights] generate_recaps`, default `true`.
  It applies to Claude panes only, and runs once per wait when all of these hold:
  * the pane is *done* or *needs you*
  * it has stayed in that same state, unseen (not the focused pane), for 180 s
  * there is no fresh `away_summary`

  It runs `claude -p --resume <sid> --fork-session --no-session-persistence --output-format json /recap`
  in the session's cwd, with the pane's env (`CLAUDE_CONFIG_DIR` and so on) and
  without bro's `BRO_*`. Other limits:
  * Only one runs at a time globally. The timeout is 120 s.
  * These results count as failures: `is_error`, an empty result, or text starting
    "Nothing to recap", "Couldn't generate", or "Recap cancelled".
  * The cache is `~/.bro/recaps/<sid>.json`, written atomically:
    `{ text, at, size, mtime, cost_usd }`.
  * Cost is about 2¢ per recap. It runs while the prompt cache is warm.
* **TUI sidebar**. Live rows get a muted second line when there is something to say:
  `⏸ <reason>` when the pane needs you, otherwise `↳ <recap>`, truncated to the row
  width. Rows with nothing to say stay one line.
* **Notifications**. The "needs input" body becomes the reason, falling back to the old
  text. The "finished" body becomes `asked`, else the recap, else the old text.

## 4. Bridge contract (bro-bridge)

`TerminalSessionSummary` gets these optional, camelCase fields. They are omitted when
empty and published through the existing `session` and `sessions` events:

```json
"attention": { "state": "blocked" | "done" | "working" | "idle", "reason": "Bash: npm publish", "since": "…Z" },
"recap": { "text": "…", "at": "…Z", "source": "claude" | "generated" | "last-message" },
"agentFiles": { "sent": 2, "touched": 14, "updatedAt": "…Z" }
```

The Rust API is `Bridge::set_insights(id, SessionInsights { attention, recap, files })`.
It is a no-op for unknown ids. It publishes only when something changed, debounced to
≤ 2/s per session.

`GET /api/sessions/{id}/agent-files` returns at most 200 entries per list, newest first:

```json
{ "sessionId": "…", "transcript": "C:\\…\\abc.jsonl",
  "sent":    [ { "path": "F:\\…\\chart.png", "relativePath": "out/chart.png", "caption": "…", "at": "…Z", "mimeType": "image/png", "isImage": true } ],
  "touched": [ { "path": "…", "relativePath": "src/app.rs", "kind": "edit", "at": "…Z", "line": 42 } ] }
```

The Rust API is `Bridge::set_agent_files(id, AgentFiles)`. It stores the lists, and the
counts go into `agentFiles` in the summary.

* `relativePath` is computed with `file_preview::resolve_path`. It is absent when the
  path is outside the roots.
* Paths outside the roots are still listed. They are previewable only if the resolver
  allows them.
* A session with no data returns `{ sent: [], touched: [] }`. An unknown session
  returns 404.

## 5. Clients

* **Session lists** (web sidebar and mobile sessions screen) show the reason
  (`⏸ …`) or recap (`↳ …`) as a muted second line.
* **The session view** gets an "Agent files" button with a count badge (sent + touched).
  It opens a list with two sections: **Sent to you** (shelf, with caption and image
  thumbnail when `isImage`) and **Touched** (kind icon, relative path, time-ago). Tapping
  an item opens the existing file preview at `line`.
* Clients refetch `agent-files` when the panel is open and `agentFiles.updatedAt`
  changes. Older hosts return 404 for that route, and the button is hidden.

## 6. llmtrim (bro-proxy, optional)

### Settings

Settings live in `v2.toml`:

```toml
[proxy]
trim = false            # off by default
trim_route_native = true
trim_level = "safe"     # or "lossless"
```

* **Turning it on:** use the palette's "compress requests (llmtrim) on / off" (`toggle_trim`) or press `c`
  in the Proxy view. The change applies to the running proxy at once (`ProxyHandle::set_trim`).
* **Native routing:** while trim is on and the proxy runs, newly launched native Claude logins and Codex
  logins are routed through bro-proxy as their own login: `claude:<p>` becomes `ClaudeOAuth`, and
  `codex:<p>` becomes `ChatGptCodex`. This is the same path pool and other-login launches use.
  `CLAUDE_CONFIG_DIR` / `CODEX_HOME` are unchanged, so transcripts stay put. Claude's model slots aren't
  pinned. Running sessions are unaffected.

### Dependency

`llmtrim-core =0.13.7` (MPL-2.0), with default features off: no tree-sitter, tiktoken or image.
It turns on serde_json `arbitrary_precision` (and `preserve_order`, which bro already used) for the
whole binary. The full workspace test suite passes with both on.

### Levels

Every lossy or behaviour-changing stage is off: `auto`, output shaping, tool selection, tool trimming,
schema minify, n-gram, retrieve, skeleton, json_crush, base64, the cache stage, images, and exact-line
dedup (which also merges non-adjacent lines).

* **safe**: tool output loses ANSI escapes and carriage-return progress frames, which is what a terminal
  would show. Toolout runs with `min_lines = MAX`, so it never drops a line.
* **lossless**: also does JSON minify and columnar (TOON) arrays. The agent then sees minified JSON.

### Mechanics

* **Where it runs:** on the inbound JSON, before translation and dispatch, and also on `count_tokens`.
  It runs in `spawn_blocking`, and bodies over 16 MB are skipped.
* **Never worse:** errors pass the original through, a result that isn't smaller forwards the
  original, and an unchanged body goes out as received.
* **Turn memo:** keeps every earlier message byte-identical across turns. Entries are recorded only
  after the upstream accepted the request. The memo salt includes a hash of the request's original
  system/instructions/tools. A changed envelope (an MCP server connecting, a model switch) therefore
  starts cold, instead of llmtrim's first-message-keyed envelope replay putting the old tool list back.
* **Reporting:** `ProxyEvent.trim_tokens_before/after` hold llmtrim's estimates. The Proxy view adds a
  "saved" column (`—` when it didn't run) and a total.

### Measured savings

Measured on a request rebuilt from a real 163-message Claude Code transcript (71 tool results), with
`measure_fixture` (ignored test). Rows marked "not used" are llmtrim's lossy presets, which bro never
enables, shown for comparison only:

| shape | safe | lossless | agent (not used) | aggressive (not used) |
|---|---|---|---|---|
| Claude Code (cache markers) | 0.00 % | 0.00 % | 0.00 % | 0.00 % |
| same conversation, no markers (Codex / Pi shape) | 0.01 % | 0.02 % | 7.6 % | 43 % |

* **Claude Code:** caches its whole history, so there is almost nothing safe to compress. Real Claude
  Code tool output is also already free of ANSI.
* **Overhead:** about 30–65 ms of CPU per 120k-token request.
