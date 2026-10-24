# Sounds (PeonPing)

Voice lines from games when an agent starts, finishes, fails or needs you: the Orc Peon's "Work, work",
GLaDOS, Navi, StarCraft units and 370+ other packs from the [PeonPing](https://peonping.com) registry,
in the [CESP](https://github.com/PeonPing/openpeon) format. In Claude Code, Codex, Pi and omp sessions
bro starts. Off by default.

Open **Sounds** from Settings › Integrations, or the palette ("sounds"). The view has:

- the switch (`s`), the volume (`-` / `+`) and a test line (`t`)
- the moments that speak (`1`–`8`): session starts, task acknowledged, task done, needs your input, something
  failed, context compacting, rapid prompts (the "annoyed" easter egg), session ends. A dimmed chip means the
  pack in use has no lines for that moment.
- a switch per harness (`c` Claude Code, `x` Codex, `i` Pi, `m` omp) and who plays its sounds
- the packs: **Installed** and **Browse** (`tab`). `/` searches by name, game, author or tag. `enter` uses a pack,
  installing it first if needed. `p` previews it (each press plays another line, before you install). `u` updates
  it when the registry has a newer version. `d` removes it. `r` fetches the registry again.

## Where things live

bro shares peon-ping's files when peon-ping is installed, so `peon packs use …` and bro agree:

| | With peon-ping | Without |
|---|---|---|
| config (`config.json`: volume, pack, categories, rules) | `$CLAUDE_PEON_DIR`, else `~/.claude/hooks/peon-ping` | `~/.bro/peon` |
| packs | `…/packs/<name>/openpeon.json` + sounds | `~/.bro/peon/packs` |
| state (last line per moment, debounces) | `…/.state.json` | `~/.bro/peon/.state.json` |

bro reads peon-ping's whole config: `default_pack` (or `active_pack`), `pack_rotation` and
`pack_rotation_mode`, `path_rules`, `ide_rules` (ids `claude`, `codex`, `pi`, `omp`), `exclude_dirs`,
`categories`, `volume`, `annoyed_threshold` / `annoyed_window_seconds`, `silent_window_seconds` and
`session_start_cooldown_seconds`. Keys bro doesn't use stay as they are when it saves.

In `~/.bro/v2.toml`:

```toml
[peon]
on = true      # sessions bro starts get the hooks
claude = true  # per harness
codex = true
pi = true
omp = true
```

The switch in Sounds sets `on` here and `enabled` in `config.json` together. So turning sounds off also
quiets peon-ping's own hooks outside bro.

## How bro wires it

bro writes into no harness's own config. Each launch gets the hooks for that one session:

- **Claude Code**: `bro hook peon claude` on SessionStart, SessionEnd, Stop, Notification, PermissionRequest,
  PostToolUseFailure, PreCompact, UserPromptSubmit and SubagentStart, in the launch's `--settings` file (exec
  form: no shell, so no quoting).
- **Codex**: the same as `-c hooks.<Event>=…` overrides for SessionStart, UserPromptSubmit, PermissionRequest,
  PreCompact, SubagentStart and Stop. Codex normally runs a new hook only after you review it in `/hooks`. bro
  also passes `-c hooks.state=…` with each hook's trust hash, worked out the way Codex does, so they run
  without a prompt. Codex runs hook commands in your shell (PowerShell on Windows), so the path to bro goes in
  bare (in its 8.3 short form if it has spaces).
- **Pi and omp**: bro's extension (`~/.bro/peon-ext/bro-peon-<hash>.ts`) with `-e`. It turns their lifecycle
  events into the same hook events.

A login whose own config already runs peon-ping gets nothing extra, so no line plays twice. That covers a
Claude `settings.json` with peon-ping's hooks, a Codex `config.toml` / `hooks.json` with its hooks or `notify`
adapter, and peon-ping's own Pi or omp extension. Sounds shows these logins. For Claude logins,
**Let bro play them** (`o`) swaps peon-ping's hook entries in that `settings.json` for bro's, so sessions
started outside bro play the same way. The file as it was is kept beside it as
`settings.json.before-bro-peon`. peon-ping's `hook-handle-use` entry goes too: its "use pack" skill stops
working in those sessions.

## How a sound plays

`bro hook peon <harness>` reads the event, maps it to a moment the way peon-ping does, picks the pack and a
line (never the same line twice in a row), saves the state and returns. A lock keeps a turn's simultaneous
hooks from losing each other's state. It hands the line to `bro hook peon play <file> <volume>`, a process
of its own, so the agent never waits.

The player decodes the file in-process and plays it on the default output device. It handles WAV, MP3,
OGG Vorbis, FLAC and AAC / M4A on Windows, macOS and Linux alike. Unlike peon-ping's Windows player, MP3 packs
need no ffmpeg and the volume applies to WAV too.

## The registry

`https://peonping.github.io/registry/index.json`, cached a day in `~/.bro/cache/peon-registry.json`.
A failed fetch falls back to the cache. Installing a pack:

1. fetches its `openpeon.json` from the pack's GitHub source and checks it against the registry's
   `manifest_sha256`
2. fetches every sound it lists and checks each against its `sha256`
3. writes everything to a staging folder, then renames it to `packs/<name>`

So a failed or tampered download leaves nothing behind. Pack names and file paths are checked: nothing lands
outside the packs folder. Previews of packs you haven't installed are cached in `~/.bro/cache/peon-preview`.
