# Focus View rules

What Focus View supports and where each rule lives. Keep this file current when any of it changes.
User-facing behaviour is described in full in `docs/gui.md` ("Focus mode").

## Surfaces

| Surface | Code | Gets its data from |
|---|---|---|
| Desktop GUI | `crates/bro-tui/src/focus.rs` (state), `app/focus.rs` (keys), `app/gui/focus/` (drawing) | `Conversation` in process |
| Phone | `F:\bro-cli-mobile\src\components\focus/`, `lib/conversation.ts`, `useConversation.ts` | bridge JSON |
| Web | the phone app through react-native-web | bridge JSON |
| Terminal UI | not supported | |

Only Claude Code and Codex sessions have a Focus View. Pi, omp, Antigravity, shells, a queued resume and a failed start keep their terminal (`focusable` in `app/focus.rs`).

## Pipeline

1. **Parse** — `crates/bro-core/src/sessions/conversation.rs` reads transcript records one line at a time (`Conversation::ingest`). Claude and Codex have separate readers; Codex `item_completed` events win over `response_item`s once one is seen.
2. **Word** — `conversation/wording.rs` turns tool calls into labels.
3. **Follow** — `sessions/subagents.rs` (`Follower`) reads each subagent's own transcript; `sessions/tasks.rs` tracks background tasks.
4. **Serve** — `crates/bro-bridge/src/conversation.rs`: `GET /api/sessions/{id}/conversation?since=<rev>&limit=<n>`.
5. **Present** — status lines, summaries and layout, written once in Rust (desktop) and once in TypeScript (phone/web).

Desktop keeps narration labels and pasted-picture matches across frames. When a transcript changes, it refreshes the affected turn and the turns after it. Switching conversations or changing the gallery refreshes picture matches.

## Conversation model

| Item | Meaning | Rules |
|---|---|---|
| `You` | what you sent | Harness wrapping stripped (`CLAUDE_NOISE`, `CODEX_NOISE` in `sessions/describe.rs`). Slash and `!` commands kept as typed, `command: true`. Pictures counted, paths kept when known. |
| `Said` | the agent's words | Markdown as written; adjacent text blocks joined. |
| `Did` | tool calls between two `Said` | One `Step` per call, in order. |
| `Note` | `interrupted`, `compacted`, `error`, `worked`, `subagent` | See Notes. |

Left out: thinking text, tool output, injected context. Kept: the latest reasoning heading (`thought`, Codex), model, effort, cwd, branch, harness version.

Limits: 400 items kept (`ITEMS_MAX`), step label 90 chars, step detail 600 chars, 200 subagents and 100 tasks remembered, 64 subagents followed.

## Steps

- Kinds: `read edit create delete run search web agent plan ask send skill mcp other`. States: `running done failed`.
- A step carries `label`, `detail` (full command, path or query), lines `added`/`removed`, `line`, time, and an MCP app when its server gave the result a page.
- Labels (`wording.rs`): a skill by its name ("Generate voice"); an MCP tool as "what · to what" with the server in `detail`; common shell commands by what they did (git, cargo, js runners, gh, docker, scripts); known names keep their spelling (`SPELLED`).
- Run summary: one step shows its own label. A running step shows `label…`, then `label · 1m 05s` after 5s (only while the agent works). Otherwise counts joined by ` · ` in the order first seen: "Read # files", "Changed # files" (edit, create, delete), "Ran # commands", "Searched # times" (search, web), "Checked in with subagents # times", "Used <server>", "Used # tools". A skill always leads.
  - Desktop: `summary` in `gui/focus/mod.rs`. Phone: `stepsSummary` in `lib/conversation.ts`.

## Status

| Rule | Behaviour |
|---|---|
| Mood | Working, Blocked ("Needs you", or "N questions waiting" for Codex queued questions), Done (shown 8s after a turn ends), Idle (nothing shown). A usage limit forces Blocked and shows the limit's title. |
| Thinking | The agent's own heading while it is the newest thing; else "Thinking" (under 12s), "Still thinking" (12–44s), "Deep in thought" (45s+). Time appended from 12s. |
| Doing | The running step's label, with time from 5s; else the thinking line. |
| Background work | Turn over but subagents or tasks still running reads as Working: "2 subagents working", a lone task's "title · time", "2 background tasks running", "1 subagent and 2 tasks working". |
| Clock | `42s`, `1m 05s`, `1h 02m`. |

Desktop: `mood_of`, `thinking`, `doing`, `header_status` in `gui/focus/mod.rs`, `working` in `gui/focus/subagents.rs`. Phone: `focusMood`, `thinkingLine`, `doingLine`, `backgroundWorkLine` in `lib/conversation.ts`.

## Notes

- `interrupted` → "Stopped". `compacted` → "Earlier conversation summarised". `error` → its text, or "The turn stopped on an error".
- `worked` → "Worked 4m 5s", only for turns of 20s or more.
- `subagent` → "<title> finished" where the agent heard back. Desktop only.
- `/model x` and `/effort y` messages read as "Now on Opus 5.5" and "Thinking high" (`setting` / `settingLine`).

## Replying

- Sent messages show at once from the draft (`Draft.sent`) until the transcript holds them (`holds`: same words or containing them, whitespace aside), for at most 15 minutes.
- Captions: "Sending…", "Waiting for the agent…", "Queued · waiting for the agent", "Not sent · check the terminal".
- Pictures hold their place in the text with an invisible token (U+E0100–E01EF); deleting the token moves the picture to the end.
- `/` lists the agent's commands. Commands that open their own screen (`opens_screen`) show the terminal while in use.
- `:` lists the Quips that match, then **New…** (`gui/focus/quips.rs`). New… opens the Quip editor with the typed shortcut filled in; saving puts the new Quip in the draft where the query was, and Esc goes back to the draft. With nothing matching, New… is click-only so Enter still sends.
- Up on an empty box recalls earlier messages. Esc on an empty box stops a working agent.

## Desktop dictation

- In a Focus View reply box, the microphone or Ctrl+Shift+D (Command+Shift+D on macOS) starts dictation. If transcription is off, Bro opens the Composer dictation settings.
- Finish inserts text at the caret or over the selection captured at the start. Bro keeps attachments added during recording and inserts after the selection if the draft changed.
- Escape or Cancel discards the take. Send finishes the take and sends the combined draft after insertion. If a choice appears before then, the text stays in the draft.
- The recording belongs to the original session. Closing that session or turning off transcription cancels it. Dictation and optional cleanup run in the desktop app with the configured OpenAI key.
- In a native agent terminal, the microphone moves to the session header. Pi, omp, Antigravity, and the Claude Code or Codex terminal can receive the transcript in the CLI's prompt. The CLI owns its editor and selection.

## Desktop breadcrumbs

- When one chat fills the workspace, its working folder and Git branch sit beside the chat title in the session header, on the same row as the session controls. The breadcrumb trail shortens from the middle as space runs out.
- Hover the trail for the full path. Clicking it opens the working folder when Bro can resolve it.
- The session header can open Focus View or return to the terminal. Split chats and headers with too little room keep the folder line inside the conversation.

## Choices

- Read from the live terminal screen, not the transcript: `bro_core::terminal_prompt::choice` (numbered single-select). Other interactive forms stay in the terminal.
- Desktop draws the options as buttons; keys go to the session's own menu. Phone gets the same choice from `crates/bro-bridge/src/prompt.rs` (plus `prompt/codex.rs`).
- A visible choice takes priority over the reply box; the draft is kept and never sent into a choice.

## Model and thinking

- Shown from the transcript's `model` and `effort`. A picked change shows at once and is typed into the session when it is free.
- Claude: `/model`, `/effort`. Codex: `/model <model> --reasoning=<level>` when its turn ends, 0.165+ only.
- Model list depends on what the session was launched on (`model_source` in `app/focus.rs`).
- Claude's list is the newest model of each family Anthropic lists for your Claude login (`GET /v1/models`, cached in `~/.bro/claude-models.cache.json`, asked again after 3 hours; `catalogue::claude_choices_from`). A new model shows on desktop and phone without an update; bro's own list stands in until it has asked. Codex's comes from the login's `models_cache.json`.
- Phone: `GET/POST /api/sessions/{id}/focus-controls` → `app/focus/remote.rs`. Refused while a choice is showing.

## Subagents and background tasks

- Subagents: Claude's Agent tool (forks and background agents included) and Codex's spawned agents. States: `running done failed stopped`.
- Transcripts: Claude `<session>/subagents/agent-<id>.jsonl`; Codex its own rollout thread. Codex briefs are encrypted, so a Codex subagent shows its nickname and work, not its brief.
- Tasks: Claude Code only — `run_in_background` commands, Monitors, workflows. A task ends on a `<task-notification>` with a status, a `TaskStop`, a Monitor timeout, or when the session's process ends.
- Desktop: a card where each started, a strip over the reply box, and each subagent's own conversation in place (Esc or **Conversation** goes back).
- Phone: the strip only (`BackgroundWork.tsx`), with each subagent's current activity computed by the bridge.

## Media, apps and links

- Pictures in messages and steps show as thumbnails; sound files show a card with waveform (`gui/focus/sound.rs`, `SoundCard.tsx`, `/api/sessions/{id}/waveform`).
- MCP Apps: a result naming a page (`_meta.ui.resourceUri` or `openai/outputTemplate`) renders that page under its step. Phone loads it from `/api/sessions/{id}/apps/{call}`. Not on Linux.
- Links and file paths open as in the terminal; a step about a file opens it at its line.
- The desktop conversation menu shows **Copy** when text is selected, **Paste**, and **Fork conversation** for the main conversation. Use the arrow keys and Enter to pick an action, or Esc to close the menu.
- **Fork conversation** is also in the session header and tab menus. Once Bro has loaded the history, it opens a new tab with a copy of the current Claude Code or Codex conversation. The original session keeps running.
- Desktop imported T3 themes use `messageSurface`/`messageForeground` for sent bubbles and
  `codeBackground`/`codeForeground` for code; terminal colors remain separate. Other themes keep the existing
  colors. Embedded browser CSS is retained on export but is never executed by the native renderer. The bundled
  Aurora Studio stylesheet has a hand-written native counterpart: palette-derived ambient gradients (one radial
  gradient quad per glow, never stacked rings: they redraw with every frame) and full
  rounded borders on messages/code. Its copy remains active under color edits while that stylesheet is unchanged.
- The floating theme editor previews unsaved palettes across the app. Its semantic inspector can target sent
  bubbles and code blocks without activating their links. A focused editor field takes typing/paste and hides
  the reply box's caret; Cancel restores the starting theme. These editor controls are desktop-only.

## Wire contract (phone and web)

- `rev` changes whenever anything moved; `?since=<rev>` answers `{ rev, unchanged: true }`. Revs never repeat across bridge restarts.
- `limit` defaults to 150, capped at 400. The phone pages 60 at a time; the desktop shows 80 and adds 80 per "Earlier".
- Item keys stay stable as the conversation grows (`you:<ms>`, `said:<ms>`, `did:<first step id>`, `note:<kind>:<ms>`, `#n` for repeats).
- `skill` and `mcp` steps go out as `kind: "other"` with `family: "skill" | "mcp"`, for older apps.
- `subagent` notes are not sent.
- TypeScript types in `lib/conversation.ts` are hand-written to match `to_json`.

## Known differences

| | Desktop | Phone / web |
|---|---|---|
| Thinking timer starts from | newest transcript record | newest item's time |
| Level "none" | "off" | "none" |
| Subagent cards, notes, own conversation | yes | no |
| Model name | `model_name` in `focus.rs` | `modelName` regex in `lib/conversation.ts` |
| New Quip from the `:` list | yes | no (`GET /api/quips` is read-only) |

## Changing a rule

1. Parsing or wording: change `bro-core/src/sessions/`, add a test there. Both surfaces pick it up.
2. Anything under Status, Steps summary, Notes or Replying: change the Rust function and its TypeScript twin named above.
3. New field: add it to `to_json` in `bro-bridge/src/conversation.rs` and to the types in `lib/conversation.ts`. Only add; older apps must keep working.
4. Update this file.
