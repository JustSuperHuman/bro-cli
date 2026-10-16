# Watchdogs

A watchdog keeps one agent session moving through a list of milestones while you're away.

## Starting one

Focus a Claude or Codex session and press `ctrl+space w` (or pick "watchdog" in the palette). Write the milestones in order, one per field. Enter starts the next one, `alt+enter` adds a line break inside one, `alt+↑/↓` reorders and `ctrl+d` deletes. Pasting a list (one item per line, bullets are fine) fills in several at once. Below the list:

- **Wake after**: how long the session sits idle after it stops before the watchdog wakes. A question on screen waits at least 2 minutes.
- **Fresh context**: the reviewer may clear the agent's conversation (`/clear` in Claude Code, `/new` in Codex) before a new milestone and then send a task that stands on its own.
- **Second opinions**: the reviewer may open a helper session on another agent, for example an Opus refinement pass over UI work Codex did. Helpers count as work: the watchdog waits while one runs and lists them in later briefs.
- **Budget**: wake-ups before it stands down.

`ctrl+s` starts it. If the session is idle at that moment, the first review starts right away.

## What happens on a wake-up

bro wakes the watchdog when the agent finishes a turn and stays idle for the wait you set, or when it has waited on a question for a while. It wakes once per stop: a review that sent nothing doesn't fire again until the session does something. A failed review is retried after 2, 5 and 15 minutes.

Every wake-up goes to the orchestrator, bro's one session that runs the others. With none running, bro starts one: the way it last ran, else the last one you launched, else on the Claude or Codex login with the most usage left. bro writes the review brief to `~/.bro/orchestrator/watchdog/` and types a one-line `[watchdog · <session>]` message pointing at it into the orchestrator. The orchestrator reaches the session through bro's own tools (the bridge's `/mcp`), so the bridge has to be on; bro turns it on when it starts the orchestrator.

The orchestrator gets the watchdog prompt and a brief: the session, the milestones with their status and notes, any helper sessions and the last few wake-ups. It can read the chat, the screen and the project (Read, Grep, Glob and read-only git), mark milestones with `watchdog_update`, clear the agent's context with `clear_context`, answer a prompt the plan settles, open a helper with `open_session` (`model` and `forWatchdog`), and finally send one task with `send_input`. The last line of its reply is the report you see in the toast and the log, with "the orchestrator" as the reviewer.

## Watching it

The session header shows `watchdog 2/5` (milestones done), a spinner while a review runs, or paused / done. `ctrl+space w` on a watched session opens its status page: milestones as the reviewer marked them, what the reviewer is doing right now, and the log of earlier wake-ups. From there `w` wakes it now, `p` pauses or resumes, `e` edits the plan (statuses and the log carry over) and `s` twice stops it. Closing the session ends its watchdog.

## The prompt

Settings → watchdog → Watchdog prompt opens an editor for the instructions every reviewer starts with (`ctrl+s` saves, `ctrl+r` loads bro's default). It's stored as `[watchdog] prompt` in `~/.bro/v2.toml`. The same section sets the defaults for new watchdogs (wake after, budget). The reviewer login and model settings only applied to the headless reviewers bro used before the orchestrator took the reviews over.
