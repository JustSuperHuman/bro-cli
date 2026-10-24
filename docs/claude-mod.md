# Claude Code in bro: the theme and the mod

Two things make Claude Code feel at home in bro. Both are under Settings › Claude Code and apply to Claude
sessions launched afterwards.

| Setting | Default | What it does |
|---|---|---|
| bro theme in Claude | on | Claude Code wears your bro theme, every colour, and follows along live as you switch or preview themes |
| bro mod | off | A Claude Code mod (a plugin of function hooks) that links the session to bro |

Saved in `~/.bro/v2.toml`:

```toml
[claude_mod]
on = false     # load the mod into new Claude sessions
theme = true   # Claude Code takes bro's theme (works with the mod off)
flair = true   # the mod: spinner and turn line in the theme's voice
hud = true     # the mod: login and limits at the end of the hint line
radar = true   # the mod: a status line when another session waits on you
tools = true   # the mod: tools Claude can call
```

## The theme

bro writes its theme as a Claude Code custom theme, `<claude config>/themes/bro.json`, in `~/.claude` and in
every pooled account's config dir. Each Claude launch selects it with `"theme": "custom:bro"` in its
`--settings` file, so your own Claude theme setting is untouched outside bro.

- Claude Code watches its themes folder, so a theme change in bro (a preview too) recolours running sessions.
  It only watches a folder that existed when it started; bro writes the file before each launch.
- The mapping lives in `crates/bro-core/src/claude_theme.rs`. Claude's own colours keep their parts (its
  orange on the mascot and the spinner, the blue of its questions, the violet and teal of its modes, green, red
  and amber for how things went), each in the theme's version of that hue: the theme's own orange where it has
  one, else its red warmed toward its yellow; never just bro's accent, so Claude stands apart from bro's
  chrome. Each is then made to stand out on the theme's background: 4.5:1 for Claude's colours and 4:1 for
  hints. On light themes the orange sits near 3:1, as Claude's own light theme has it, so it stays orange rather
  than turning brown.
- Text, frames and surfaces are the theme's own: your prompt sits on its selection tint, and diffs are its green
  and red washed into its background. The `terminal` theme keeps Claude's own ANSI colours (`dark-ansi`).
- The file carries a `bro` block Claude Code ignores: the theme's id, label, marker and palette, for the mod.
- `BRO_THEME_DUMP=<dir> cargo test -p bro-tui dump_claude_themes -- --ignored` writes every built-in theme as
  Claude would get it.
- Pass your own `--settings` and bro adds none: no bro theme then.

## The mod

Source in `crates/bro-core/claude-mod/`, built into bro and written to `~/.bro/claude-mod/bro-<hash>/` (one
folder per build, so a running session keeps the copy it started with). bro passes `--plugin-dir` and the
options as `pluginConfigs.bro.options`. Outside a bro session (no `BRO_SESSION_ID`) every hook passes through.

Two halves:

- **style/**: the spinner's word and the turn line's past tense in the theme's voice ("Forging…", then
  "Forged for 12s" on Ember; "Decrypting…" on Matrix). Families per theme in `hooks/style/words.ts`.
- **link/**: the login, its 5-hour and weekly use and the context fill at the end of the hint line; a status
  line when another session needs you; `/bro` (a pane of every session, most urgent first, a press away from
  each, and every login's limits; `/bro go <session>`, `/bro title <text>`, `/bro limits`); and tools for
  Claude: `mcp__bro__show_file` (bro's preview), `mcp__bro__notify_user` (desktop and phone) and
  `mcp__bro__bro_sessions` (list or read the other sessions). The orchestrator gets `show_file` only: its
  `bro` MCP server has the rest under the same names.

The link talks to the bridge of the bro that launched the session (`BRO_BRIDGE_URL`). Without one (the bridge
off, or a second bro opened beside a running one) it stays quiet rather than ask another bro.

bro also tells every Claude launch `BRO_PROFILE` (the login whose config dir it runs in), `BRO_BACKEND` (who
serves the model: the login, a provider id, `pool`) and `BRO_VERSION`.

Working on the mod:

```sh
claude plugin validate crates/bro-core/claude-mod
claude --plugin-dir crates/bro-core/claude-mod     # with BRO_SESSION_ID and BRO_BRIDGE_URL set
```

Every file the mod holds must be listed in `claude_mod::FILES` (a test checks).
