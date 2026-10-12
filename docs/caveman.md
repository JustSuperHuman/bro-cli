# Caveman mode

Terse replies that keep every technical fact: answer first, no filler, code, paths and errors verbatim.
The rules come from [Caveman](https://github.com/JuliusBrussee/caveman) by Julius Brussee (Apache-2.0),
adapted so that bro owns the switch. Vendored in `crates/bro-core/src/caveman/rules/` (see `NOTICE` there).

| Level | Style |
|---|---|
| caveman | Full but terse sentences. Answer first, no ceremony. |
| ultracave | Fragments, each fact once. The shortest replies. |
| megacave | Classical Chinese (文言文). Code and identifiers stay as they are. |

Turn it on in Settings (the ⚙ in the sidebar, `alt+,`), or in the palette: "caveman mode ultracave",
"caveman off", or the `toggle_caveman` action (no default key; bind one under `[keys]`). Changes apply
to sessions launched afterwards. While it's on, the sidebar's top border shows `⛏ <level>`. Every
session launched with it shows `⛏ <level>` in its header.

Saved in `~/.bro/v2.toml`:

```toml
[caveman]
on = true
level = "ultracave"   # caveman | ultracave | megacave
subagents = true      # Claude subagents get the rules too
```

## How bro applies it

bro installs nothing: no plugin, no Node, no edits to the harness's settings files. Each launch gets:

| Harness | How |
|---|---|
| Claude Code | The rules go in the system prompt via `--append-system-prompt-file ~/.bro/caveman/<level>.md`. That holds on every request, survives compaction and works on every backend (pool, providers, bro-proxy). A `--settings` file adds two hooks. They print ready-made JSON from `~/.bro/caveman/` with `cat`. `SubagentStart` hands subagents the rules. `SessionStart` (resume) restates the mode, because Claude Code reuses a resumed conversation's recorded system prompt until it compacts. |
| Codex | `-c developer_instructions=…`. Any `developer_instructions` already in the login's `config.toml` come first. |
| Pi / omp | `--append-system-prompt <rules file>` |

Details:

- Claude Code keeps only the last `--append-system-prompt-file`. bro merges its browser briefing, any
  you pass yourself and the caveman rules into one file (`~/.bro/caveman/prompts/<hash>.md`).
- If you pass your own `--settings`, yours wins. The rules are still in the system prompt, but
  subagents and resumes don't get the hooks.
- Turning caveman off doesn't change sessions that already run. A Claude session *resumed* after you
  turned it off is told caveman is off. That note is only sent once caveman has ever been on.
- The upstream Caveman plugin can stay installed. While bro's mode is on, bro sets
  `CAVEMAN_DEFAULT_MODE=off` so the plugin doesn't inject a second copy. Its `/caveman` commands still
  work. Settings notes when the plugin is installed in `~/.claude`.
- Saying "stop caveman" or "normal mode" in a session switches that session back to normal prose. The
  header badge can't see that. It shows what the session was launched with.

Not included: Caveman's proxy (bro-proxy already handles request compression; see Settings →
Request compression), `caveman browse`, `/caveman-compress` and the other upstream skills.
