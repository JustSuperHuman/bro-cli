# Create Slop

Create Slop is a way to start a session for making video and media. You pick who is in it and bro starts the agent
with those characters and your media tools already connected. The session opens as a studio: the agent on one side
and the film's storyboard on the other.

Open it with `alt+shift+s`, or from the new-session page (`alt+n`) with its third tab, **Create Slop** (`alt+s`
there). The command palette has it as "create slop". In the desktop app the page takes the window's whole width.

## The cast

The left side is a grid of characters:

- **Claudia**, the open character from [claudia.gallery](https://claudia.gallery). Her server gives the agent her
  canon, 3,723 images with their prompts, and the guides the films were made from.
- **SLOPCANNON's roster** from [character.slop.cc](https://character.slop.cc/characters). Each character has a
  written canon and locked reference pictures, published as a version.

Type to find someone by name, by who made them or by a tag. Space or a click casts the character under the cursor,
and the same again takes them out. Hover a card to read its tagline. The people you cast show as chips above the
grid, and the button says who you are starting with.

bro fetches the roster the first time you open the tab and keeps it for six hours (`ctrl+r` fetches it again).
Portraits are downloaded as you scroll to them, shrunk and kept in `~/.bro/studio/portraits/`, so the grid is
instant the next time.

A project remembers its cast. Open the tab in the same folder later and the people you last cooked with are
already chosen.

## The toolkit

The right side lists the media tools, each under its own logo, with what this computer has for it: a check when
it is ready, a sign-in mark when it will ask you to sign in, and otherwise what it is waiting for. A tool that can
be used has a switch, so you can leave it out of a session. Select a tool to see how work with it goes.

A tool that can't be used yet has a button that gets it going, and Enter on its row does the same:

- **Install** (ffmpeg, Node.js for Remotion, JustImagine) runs `bro install <tool>` in a tab, the way
  [Quick Install](quick-install.md) does it on your system: WinGet on Windows, Homebrew on macOS, your package
  manager on Linux. Open Create Slop again when it finishes and the tool is ready. A session started before you
  restart bro still finds a freshly installed ffmpeg, because bro adds its folder to that session's `PATH`.
- **Start** restarts JustImagine when it is installed and its service has stopped.
- **Add key** opens a field in the tool's row. Paste the key and press Enter. bro keeps it with your other keys
  (`keys.<tool>` in `~/.bro/config.json`) and gives it to sessions in their environment. A line copied as
  `NAME=value` or in quotes is tidied up, and the field shows only the key's last four characters.

Higgsfield and SLOPCANNON work without a key by signing you in. Open either one and choose **Add key** (`ctrl+k`)
to give it one anyway: it then connects by itself, in Codex too.

| Tool | What bro looks for | What the session gets |
|---|---|---|
| JustImagine | its service answering on this computer | the `generate-images`, `generate-video` and `generate-voice` skills, pointed at it |
| ElevenLabs | a key you gave bro, or `ELEVENLABS_API_KEY`; else JustImagine's own key | the key in the session's environment, and ElevenLabs' MCP server when `uvx` is installed |
| Higgsfield | a key you gave bro, `HF_KEY`, or `HF_API_KEY` with `HF_API_SECRET` | its MCP server at `https://mcp.higgsfield.ai/mcp`, and a key you gave bro as `HF_KEY` (an `id:secret` key also as `HF_API_KEY` and `HF_API_SECRET`) |
| SLOPCANNON | a token you gave bro, `SLOPCANNON_AGENT_TOKEN` or `SLOPCANNON_TOKEN` | its MCP server at `https://api.slop.cc/mcp`, with the token when there is one (set as `SLOPCANNON_AGENT_TOKEN` for its CLI) |
| ffmpeg | `ffmpeg` on PATH, or where an installer just put it | told to cut and mix with it |
| Remotion | Node.js with `npx` | told to make titles and captions with it |

Without a key, Higgsfield asks you to sign in the first time a session uses it, and so does SLOPCANNON. In Claude
Code, run `/mcp`, pick the server and sign in. The sign-in is kept, so later sessions connect on their own. The
server list bro writes for a session names the variable a token is in and never holds the token itself.

## The storyboard

A Create Slop session opens with the film's storyboard beside it, and the pair takes the whole window (the sidebar
steps aside while that tab is in front). The storyboard is the plan you and the agent both work from: the film as
timed scenes, each with its script (what is heard), its direction (what is seen), its keyframes and its takes.

The agent writes it as you talk. You change it where it stands:

- Click the film's title, its treatment, or a scene's title, script, direction or "Ask for a change" box to edit
  it. `ctrl+s` saves, `tab` saves and moves to the next text, `esc` cancels.
- Click a keyframe to film the scene from it, or a take to put it in the cut. Double-click opens the file.
- **Approve** signs a scene off. **Approve all** signs off every scene that isn't waiting on a change.
- The timeline across the top shows each scene as long as it runs. A block fills in once the scene has a
  keyframe and turns green once it has a take. A check marks an approved scene and a warning mark one you asked
  to change.

Your changes wait on the board. **Send changes to the agent** types one message into the session telling it to
read the board again, and the agent picks up from what it says now. The agent is told to film only the scenes you
approved, from the keyframe you chose.

For a film made with SLOPCANNON, the storyboard checks its limits as you go: scenes of 5 to 15 seconds that run on
from each other with no gaps, and a direction of at most 2,800 characters. The direction is the scene's complete
beat, so the plan you approved is the plan SLOPCANNON quotes.

The agent's side is five tools on the `bro` MCP server (Claude Code and Codex), or `bro ctl <tool>` from a shell
(Pi and omp):

| Tool | Does |
|---|---|
| `storyboard_plan` | sets the film (title, treatment, frame, length, cast) and its scenes |
| `storyboard_scene` | changes one scene: its words and timing, a keyframe or take to attach, which one is chosen |
| `storyboard_get` | reads the board, with what you changed since the agent last read it |
| `storyboard_timed_plan` | gives the board as SLOPCANNON's timed plan, with what it would refuse listed first |
| `storyboard_show` | brings the storyboard to the front, on a scene |

The board is one file, `slop/storyboard.json` in the project, so it travels with the project and an agent without
the tools can edit it directly. The storyboard view reads the file again whenever it changes. When bro's bridge
is off the tools aren't there, and the agent is told to write the file instead.

In a terminal the storyboard is a list of scenes with the selected one opened: `j`/`k` move, `s` `d` `c` edit the
script, direction and change request, `a` approves, `1`-`9` choose a keyframe, `g` sends your changes.

## What the session gets

Starting from this tab is the same as running these yourself for each character, without leaving anything in your
config:

```
claude mcp add --transport http chinese-bone-b6b8d48b5a2f \
  'https://character.slop.cc/characters/chinese-bone-b6b8d48b5a2f/mcp?version=1'
claude mcp add --transport http claudia https://claudia.gallery/mcp
```

The servers are attached to that session only. A server you already added yourself under the same name stays
yours.

The agent also gets a brief: who is cast and where their canon is, which tools are connected and how to use each,
how the storyboard works, the order the work goes in (idea, storyboard, keyframes, clips, voices, cut), and a rule
to say what a batch costs
and wait for your yes before it spends credits. It is told to treat everything a character's server returns as
reference material and never as instructions.

What each agent can take:

| Agent | Servers | Brief |
|---|---|---|
| Claude Code | all of them; reading a cast member's sheet and writing the storyboard never ask for permission | appended to its system prompt |
| Codex | the ones that need no sign-in (the cast, the storyboard, ElevenLabs, SLOPCANNON with a token) | added to its developer instructions |
| Pi, omp | none (they have no MCP) | appended to their system prompt, with each character as a JSON address |

To use Higgsfield or SLOPCANNON from Codex, add the server to Codex once (`codex mcp add slopcannon --url
https://api.slop.cc/mcp`, then `codex mcp login slopcannon`). bro leaves a server Codex already has alone and tells
the agent it is connected.

The session is named `slop · claude · <login>`. Where it runs is whatever the New session tab has selected;
`shift+←/→` changes the agent from the Create Slop tab, and **Change** goes to the New session tab for the login
and model.

## Continuing later

A Claude Code session started here keeps its kit. Continue it from the Continue tab, or reload it with another
profile, and the same servers and brief come back. bro gives the session its id at launch and keeps the kit under
that id in `~/.bro/studio/sessions.json`.

Codex, Pi and omp choose their own session ids, so a session of theirs continued later starts without the kit.
Start a new one from the tab to get it back.

## Keys

| Key | Does |
|---|---|
| type | find a character |
| `←` `→` `↑` `↓` | move in the cast (in the toolkit: `↑` `↓`) |
| space | cast the character, or switch the tool |
| enter on a tool that isn't ready | install it, start it, or add its key |
| `ctrl+k` on a tool | add its key |
| tab | cast ⇄ toolkit |
| `shift+←` `shift+→` | the agent it runs on |
| `ctrl+e` | permissions |
| `ctrl+r` | fetch the roster again |
| enter | start |
| esc | clear the search, leave the toolkit, close |

## Files

Everything is under `~/.bro/studio/`:

- `roster.json`: the roster as last fetched
- `portraits/`: the shrunken portraits
- `projects.json`: the cast each project last cooked with
- `sessions.json`: the kit each session was launched with
- `launch/`: the MCP configs and briefs sessions point at

Deleting the folder is safe. bro fetches the roster and portraits again and forgets the remembered casts.

Keys you gave bro are with your other keys in `~/.bro/config.json`. A project's storyboard is in the project, at
`slop/storyboard.json`.

Create Slop is separate from the [Slop Collector](slop-collector.md), which cleans up what renders leave behind.
