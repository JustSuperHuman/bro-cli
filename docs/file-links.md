# File links: tap a path, see the file

Paths printed by agents and shells (`src/app.rs:42`, `C:\repo\x.ts(10,4)`,
`` `crates/bro-tui/src/main.rs` ``, `[main.rs](crates/bro-tui/src/main.rs#L12)`,
`file:///C:/repo/README.md`) become clickable/tappable in every bro surface
(TUI, embedded web client, Just Terminal mobile) and open a preview at that
line. Agents can also open a file for the user with `bro open path:line`.

The bridge is the single source of truth for **whether a path is a real file**
and for **reading it**; clients only find candidates in text.

## 1. Candidate detection (clients)

Every client finds candidates with the same rules as
`bro-cli-mobile/src/lib/fileLinks.ts::findFileLinks` (Markdown link targets,
quoted/backticked paths, bare paths with a separator, bare paths with spaces
and a line suffix, bare `name.ext`, the
location suffixes `:L`, `:L:C`, `#L12`, `#L12C3`, `(L,C)`, `file://` URLs).
Wrapped terminal rows are joined into one logical line before matching; hard
newlines end a candidate.

In the mobile app, candidates can be tapped while the bridge checks them; the
file preview endpoint validates a path before reading it. Confirmed missing
paths return to plain text. Hosts that predate `links/resolve` keep every
candidate tappable.

## 2. REST (all under the existing token auth; loopback needs no token)

### `POST /api/sessions/{id}/links/resolve`

```json
{ "candidates": [ { "path": "src/app.rs" }, { "path": "README.md" } ] }
```

At most 200 candidates per request (extra are ignored). Response, same order:

```json
{ "results": [
  { "path": "src/app.rs", "exists": true, "kind": "file",
    "resolved": "C:\\repo\\src\\app.rs", "relativePath": "src/app.rs" },
  { "path": "README.md", "exists": false }
] }
```

`kind` is `"file"` or `"dir"`. Clients only link `kind: "file"`. Results are
stable for a while; clients cache by `(sessionId, path)` for ~30 s.

### `GET /api/sessions/{id}/file?path=&line=&start=&limit=`

Backward compatible with the existing preview. Without `start`, the window is
centred on `line` (as before). With `start` (1-based), it returns `limit` lines
from there. `limit` defaults to 300, maximum 5000. Markdown files with no `start` return
from line 1 (up to 5000 lines) so they can be rendered whole.

Text response (new fields marked *):

```json
{ "kind": "text", "path": "C:\\repo\\src\\app.rs", "relativePath": "src/app.rs"*,
  "language": "rust"*, "line": 42, "startLine": 22, "endLine": 321*,
  "totalLines": 900, "truncated": true, "bytes": 30123*, "content": "..." }
```

`language` is a lowercase id from the extension or file name: rust,
typescript, tsx, javascript, jsx, json, toml, yaml, markdown, python, go,
java, kotlin, swift, c, cpp, csharp, css, scss, html, xml, sql, shell,
powershell, dockerfile, makefile, ruby, php, lua, diff, ini, plaintext.

Image response: `{ "kind": "image", "path", "relativePath"*, "mimeType", "data" (base64), "bytes"*, "url"* }`.
Images are png, jpg/jpeg, gif, webp, bmp, ico, avif and **svg**. Clients must render SVG only as an `<img>` or
`Image`, never inline. A picture over 2 MB comes without `data`. Show it from `url` (`/raw`, as below).

Video/audio response, with no size limit and no file bytes in the JSON:

```json
{ "kind": "video", "path", "relativePath", "mimeType": "video/mp4", "bytes": 734003200,
  "url": "/api/sessions/{id}/raw?path=C%3A%5Crepo%5Csrc%5Cdemo.mp4" }
```

`kind` is `"video"` for mp4, m4v, mov (`video/quicktime`), webm, mkv
(`video/x-matroska`) and ogv. It is `"audio"` for mp3, wav, ogg, oga, opus, m4a, aac and flac.
`url` is relative to the host, names the file by its resolved absolute path (unambiguous whichever
root matched), and carries no token. Clients add the token as a `token`
query parameter because `<video>`/`<audio>` elements can't send headers.

Errors are `4xx` with `{ "message": "..." }`: not found, outside the workspace, binary,
too large (2 MB, text/images only), or not UTF-8.

### `GET|HEAD /api/sessions/{id}/raw?path=[&download=1]`

Streams the resolved file's bytes from disk, never buffering it whole. It uses the same
resolver and root checks as `/file`, and has no size limit. Requirements, because iOS
AVPlayer and Safari are strict:

* `Accept-Ranges: bytes` on every response.
* A single `Range: bytes=a-b`, `a-` or `-n` gets `206` with `Content-Range: bytes a-b/total`
  and an exact `Content-Length`. An unsatisfiable range gets `416` with `Content-Range: bytes */total`.
  A multi-range request gets the full `200` body.
* No `Range` gets `200` with the full length.
* `HEAD` returns the same headers with no body.
* `Content-Type` comes from the extension (the video/audio/image types above, `application/pdf`,
  text as `text/plain; charset=utf-8`, else `application/octet-stream`).
  Responses also carry `X-Content-Type-Options: nosniff` and
  `Content-Security-Policy: sandbox` (served bytes are never treated as an active page).
* `Content-Disposition: inline` by default, or `attachment; filename*=UTF-8''…` with `download=1`.
* `ETag` and `Last-Modified` come from size and mtime; `If-Range` is honoured, and
  `Cache-Control: private, max-age=0, must-revalidate`.
* Auth is the usual header, Bearer, or `?token=`, and loopback is exempt.

This is also the download endpoint for every file kind.

### `GET /api/media[?session=<id>]`

The phone's gallery, the desktop app's media panel for clients. It lists pictures, videos,
sounds and PDFs that are still on disk:

* what the agent sent you or made (wrote, created, edited, moved), or looked at;
* what was opened for you (`open`);
* what you attached (`attachments`) or sent in a message;
* what a reply names by its path.

One session's, or every session's without `session`. Newest first, each file once with its
strongest reason, at most 300.

```json
{ "items": [ { "path": "C:\\out\\take.wav", "name": "take.wav", "kind": "audio", "mimeType": "audio/wav",
  "source": "sent", "at": "2026-10-28T21:40:00.000Z", "bytes": 247004, "sessionId": "s1", "caption": "…" } ] }
```

`kind` is `image`, `video`, `audio` or `pdf`. `source` is one of `opened`, `sent`, `made`,
`viewed`, `attached` or `pasted`. `caption` appears only when the agent gave one. The files
stream from `/raw`. An unknown session is `404`.

### `POST /api/sessions/{id}/open`

Body `{ "path": "src/app.rs", "line": 42, "column": 3, "source": "agent" }`.
It resolves the path the same way (404 with `message` when missing). Then it:

* broadcasts the WebSocket event below to every client, and
* tells the bro TUI (`BridgeCommand::OpenFile`) to show its preview.

It returns `{ "opened": true, "clients": <connected ws clients> }`.

### `POST /api/sessions/{id}/open-in-editor`

Body `{ "path", "line", "column" }`. This opens the file in the desktop's editor on
the host machine (`BridgeCommand::OpenInEditor`). It is used by the web and mobile
"Open on desktop" action. It returns 204.

## 3. WebSocket event

```json
{ "type": "openFile", "sessionId": "…", "path": "src/app.rs",
  "relativePath": "src/app.rs", "resolved": "C:\\repo\\src\\app.rs",
  "line": 42, "column": 3, "source": "agent", "at": "2026-10-02T…Z" }
```

* **Web**: opens the preview panel for that file (non-modal), without stealing
  keyboard focus from the terminal.
* **Mobile**: shows an in-app banner ("Agent opened src/app.rs:42 · View")
  and opens the preview when tapped. If that session is already on screen and
  nothing modal is open, the preview opens directly.
* Older clients ignore unknown event types.

## 4. Path resolution (bridge, shared with the TUI)

The **roots** are the session cwd and the project root (`SessionMeta.project`, else
the git root above cwd). Resolution tries, in order:

1. `file://` URLs become paths. `~/` expands to home. MSYS `/c/x` becomes `C:/x` on
   Windows.
2. An absolute path is accepted only if it canonicalises inside a root.
3. A relative path is tried against cwd, then the project root, then with git diff
   prefixes stripped (`a/`, `b/`).
4. If nothing matched and the path has no `..`, a **unique** suffix match in the
   project's file index is used (the same index as `@file` search). Ambiguous matches
   count as missing.

Everything is canonicalised and must `starts_with` a root (symlinks cannot
escape). Control characters, UNC paths and non-`file` URLs are rejected.

Media is the one exception, because agents leave it outside the workspace: a scratchpad,
JustImagine's outputs, the phone's own uploads. A picture, video, sound or PDF given by its
absolute (or `~/`) path also resolves outside the roots when it's tied to the work. That means
it is any of these:

* in any session's cwd or project root, or a folder bro knows you work in (open projects,
  recent launch folders, past sessions' projects);
* in the phone's uploads folder;
* a file an agent sent, made or looked at, or one attached or opened;
* named in a conversation: a picture sent with a message, or a path in a reply.

`open` from this computer (an agent's `bro open`) counts as that tie itself. Settings ›
Session server › **Protect phone media** (`[bridge] protect_media`, on by default) turned off
lets media resolve wherever it is (`Bridge::set_media_anywhere`). A file the rule turns down
is `403` with a message that says so.

The rule applies to `/file`, `/raw`, `/waveform`, `links/resolve` and `open`; such a file has an
empty `relativePath`. The file's own extension decides, after any link, and a relative path
never climbs out. Code and text stay inside the roots either way.

## 5. Desktop (bro TUI)

* The TUI detects candidates on the visible screen alongside URLs. It checks
  existence off the UI thread and caches the result, and only confirmed files
  get the link affordance.
* Clicking a text file opens an in-TUI preview overlay: syntax highlighted, scrolled to
  and highlighting the line. Keys: `e` opens it in the editor, `c` copies the path,
  `Esc`/`q` closes, plus scrolling.
* The editor is `$BRO_EDITOR`, else `$VISUAL`/`$EDITOR`, else VS Code (`code -g
  file:line:col`), else the OS default app.
* Images, video, audio, binary and oversized files open immediately in the OS default app.
  Missing files show an error in Bro.
* `BridgeCommand::OpenFile` (the agent or a remote client asked) opens the file
  when that session is focused, else shows a status message.

## 6. Agent open (`bro open`)

`bro open <path>[:line[:col]] [--session <id>] [--editor]`:

* Runs inside a bro pane (`BRO_SESSION_ID` set by bro).
* Finds the bridge from `BRO_BRIDGE_URL` (set in pane env by bro when the
  bridge is running at spawn time), else the bridge's existing discovery file
  `<bridge data root>/.terminal-web-server.json` (`{ pid, host, port, startedAt,
  runtime: "bro" }`, written atomically on bind, removed on shutdown), read with
  `bro_bridge::read_endpoint(&BridgeConfig::default_data_root())`.
* POSTs `/api/sessions/{id}/open` (or `open-in-editor` with `--editor`) on loopback.
* Exits non-zero with a one-line reason when the file is missing or bro isn't
  running.

The `bro-open` skill is written into each launched Claude profile
(`<CLAUDE_CONFIG_DIR>/skills/bro-open/SKILL.md`) and Codex home
(`<CODEX_HOME>/skills/bro-open/SKILL.md`). It tells the agent to run `bro open`
when it wants the user to look at a file or a specific line, for example after
making a change it wants reviewed, or when answering "where is X". It should do
this sparingly: once per answer, not for every file it mentions.
