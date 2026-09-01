# JustImagine HTTP API

`bro imagine` (and `bro imagine service install`) starts a local web server for the JustImagine gallery. The UI has no private browser-only path: it calls the JSON routes documented here on that same server, so scripts can drive it exactly the way the page does.

The server binds to `127.0.0.1` and adds no API auth layer of its own. Keep it local. Your upstream API keys stay server-side, loaded from `~/.bro/config.json` or the provider's environment variable.

## Start the server

```sh
bro imagine -p openrouter              # foreground, prints its URL, opens a browser
bro imagine --root D:/Art --port 9000  # a different gallery, on a fixed port
bro imagine service install            # background, at every login, on port 8791
```

A foreground run prints the base URL and the gallery root:

```text
Gallery:  http://127.0.0.1:8790
Folder:   <cwd>/.bro/justimagine
```

Use that URL as `BASE` below. The service defaults to `http://127.0.0.1:8791`; `bro imagine open --no-open` prints it without launching a browser.

## Layout on disk

Everything lives under one root, so the whole gallery is a single movable tree:

```text
<root>/                    folder "" — generations made at the top level
  history.jsonl            metadata for the media beside it
  <name>/                  a folder you made; nests arbitrarily
    history.jsonl
  .context/                reference images, named by content hash
  .thumbs/                 derived posters/thumbnails (safe to delete)
```

Metadata is per-folder rather than one central index. That is what makes deleting a folder a plain recursive remove with nothing left dangling, and what lets a folder survive being moved by hand in Explorer or Finder.

## Routes

| Route | Purpose |
| --- | --- |
| `GET /api/state` | Everything the UI needs to boot: gallery root, image APIs, live video-model catalogue, folder tree, reference library, in-flight jobs |
| `GET /api/items?folder=<rel>` | The media in one folder, newest first |
| `GET /api/folders` | Just the folder tree |
| `POST /api/folder` | `{parent, name}` → create |
| `POST /api/folder/rename` | `{path, name}` → rename |
| `POST /api/folder/delete` | `{path}` → delete the folder and every generation inside it |
| `POST /api/move` | `{from, to, files[]}` → move media between folders, metadata and thumbnails included |
| `POST /api/delete` | `{folder, file}` → delete one generation |
| `POST /api/generate` | Queue one or more generations; returns job ids |
| `POST /api/cancel` | `{id}` → cancel a queued or in-flight job |
| `POST /api/enhance` | `{prompt, kind, model?, characters?, images?}` → rewrite the prompt |
| `GET /api/events` | Server-sent events: a snapshot of live jobs on connect, then every state change |
| `POST /api/context` | `{dataUrl}` → save a reference image (deduped by content hash) |
| `POST /api/context/delete` | `{file}` → remove one reference image |
| `GET /api/characters` | The cast |
| `POST /api/characters` | `{name, description}` → create |
| `POST /api/characters/update` | `{id, name?, description?, cover?}` → edit (renaming changes the id) |
| `POST /api/characters/delete` | `{id}` → delete the character and its references |
| `POST /api/characters/refs` | `{id, dataUrl}` to upload, or `{id, folder, file}` to promote a generation |
| `POST /api/characters/refs/delete` | `{id, file}` → drop one reference |
| `POST /api/characters/refs/generate` | `{id, count}` → draw a reference sheet; returns a job id |
| `POST /api/characters/refs/keep` | `{id, files[]}` → promote generated shots into references |
| `POST /api/characters/candidates/clear` | `{id, files?}` → discard generated shots |
| `GET /charref/<file>?c=<id>` | One of a character's reference images |
| `POST /api/thumb` | `{folder, file, dataUrl}` → cache a JPEG thumbnail |
| `GET /media/<file>?f=<folder>` | The media itself. Supports byte ranges, so `<video>` can seek. Add `&dl=1` for a download disposition |
| `GET /thumb/<file>?f=<folder>` | The cached thumbnail, or 404 if none has been made yet |
| `GET /context/<file>` | A reference image |

Folder ids are `/`-joined relative paths; `""` is the top level. Anything that tries to escape the root is refused with `Invalid folder path`.

## Generate an image

```sh
curl -s "$BASE/api/generate" \
  -H "content-type: application/json" \
  -d '{
    "kind": "image",
    "folder": "Product/Bottles",
    "api": "openrouter",
    "model": "google/gemini-3.1-flash-image",
    "prompt": "a clean product photo of a steel water bottle",
    "size": "1024x1024",
    "quality": "high",
    "count": 2
  }'
```

```json
{ "jobs": ["mtiy3mis-0-99d1", "mtiy3mis-1-4c02"] }
```

`size` and `quality` are ignored by chat-routed models (Gemini and friends) — steer those with the prompt. `api` picks which configured image API to use and defaults to the one the server started with.

## Generate a video

```sh
curl -s "$BASE/api/generate" \
  -H "content-type: application/json" \
  -d '{
    "kind": "video",
    "folder": "Campaign",
    "model": "google/veo-3.1",
    "prompt": "a slow dolly across a rain-streaked window at night",
    "duration": 8,
    "resolution": "1080p",
    "aspectRatio": "16:9",
    "audio": true,
    "seed": 42
  }'
```

Video always runs on OpenRouter's `/api/v1/videos` and needs `keys.openrouter` (or `OPENROUTER_API_KEY`). JustImagine submits the job, polls it on a ramping interval, downloads the finished clip and files it in the folder you named — the HTTP request returns as soon as the job is queued.

Only send knobs the model supports; `GET /api/state` returns each model's real capabilities:

```json
{
  "id": "google/veo-3.1",
  "name": "Google: Veo 3.1",
  "durations": [4, 6, 8],
  "resolutions": ["720p", "1080p", "4K"],
  "aspectRatios": ["16:9", "9:16"],
  "sizes": ["1280x720", "1920x1080", "3840x2160", "…"],
  "frames": ["first_frame", "last_frame"],
  "audio": true,
  "seed": true
}
```

`size` (`"1920x1080"`) is interchangeable with `resolution` + `aspectRatio`; sending `size` wins and the other two are dropped, so the upstream never receives a contradiction.

## Reference images

Upload once, then refer to the returned file name. The same picture is never stored twice.

```sh
curl -s "$BASE/api/context" -H "content-type: application/json" \
  -d '{"dataUrl":"data:image/png;base64,iVBORw0KGgo…"}'
# → {"file":"9f1c2b7a4e5d6081.png","existed":false}
```

Then pass those names as `images`:

- **Image models** — an images API routes through `/images/edits`; a chat-routed model receives them as vision input.
- **Video models** — with `firstFrame: true` (the default) and a model whose `frames` include `first_frame`, the first reference becomes the opening frame, i.e. image-to-video. Otherwise every reference is sent as `input_references` for style guidance.

```sh
curl -s "$BASE/api/generate" -H "content-type: application/json" \
  -d '{"kind":"video","folder":"Campaign","model":"bytedance/seedance-2.0",
       "prompt":"the camera pushes in slowly","images":["9f1c2b7a4e5d6081.png"],"firstFrame":true}'
```

> OpenRouter documents frame images as directly downloadable URLs. JustImagine inlines local references as `data:` URLs, which most upstreams accept; if one rejects it, use a model that takes `input_references`, or host the frame at an `https://` URL.

## Improve a prompt

The ✨ button beside either text field, and the same thing over HTTP. It runs server-side on `google/gemini-3.7-flash` using the OpenRouter key, so nothing extra is configured.

```sh
curl -s "$BASE/api/enhance" -H "content-type: application/json" \
  -d '{"kind":"video","model":"google/veo-3.1","prompt":"rain on a window"}'
```

```json
{ "prompt": "A slow push-in macro shot frames a clear glass window pane during a steady rainstorm…",
  "model": "google/gemini-3.7-flash" }
```

`kind` picks the job, and each gets a different instruction:

| `kind` | Written for | Asks for |
| --- | --- | --- |
| `image` | the model in `model` | subject, composition and framing, light, colour, material detail — under ~80 words |
| `video` | the model in `model` | one continuous shot with a named camera move, and what changes across it — under ~110 words |
| `character` | a saved character's description | the permanent look only — age, build, face, hair, skin, signature clothing; no pose, place or lighting |

The rewrite is shaped by what the composer actually has:

- **The model's own capabilities.** A video model's supported durations become "the clip is short (4–8 seconds), so describe one beat"; a model that produces no audio is told not to describe sound.
- **The picked cast.** Their names are kept verbatim and the enhancer is told *not* to restate their face or clothing — the reference images already carry that.
- **Attached references**, by count, so the rewrite describes the scene instead of the picture you already handed it.

Pass `characters` and `images` exactly as you would to `/api/generate`.

> `gemini-3.7-flash` is a reasoning model: it spends ~350 tokens thinking before writing, which is why the budget is 1200. If a reply still runs out of room, the answer is trimmed back to its last complete sentence and returned with `truncated: true` rather than handing you a dangling clause.

Set `JUSTIMAGINE_CHAT_URL` to point the enhancer at a proxy; it is read per call, so a running service picks it up without a restart.

## Characters

A repeatable cast: a name, a description and up to 8 reference images, saved once and picked per generation. The library is global — `~/.bro/justimagine/characters` — so it is the same in every gallery and in the background service, and a character owns its reference images rather than pointing into a gallery that might be deleted.

```text
~/.bro/justimagine/characters/
  nora/
    character.json      { name, description, cover, ts }
    refs/<sha>.png      its own reference images, deduped by content hash
```

```sh
curl -s "$BASE/api/characters" -H "content-type: application/json" \
  -d '{"name":"Nora","description":"early 30s, short dark curly hair, freckles, green field jacket"}'
# → {"character":{"id":"nora","name":"Nora","refs":[],"cover":"","description":"…"}}

# add a reference, either by upload…
curl -s "$BASE/api/characters/refs" -H "content-type: application/json" \
  -d '{"id":"nora","dataUrl":"data:image/png;base64,iVBORw0KGgo…"}'

# …or by promoting a generation you liked
curl -s "$BASE/api/characters/refs" -H "content-type: application/json" \
  -d '{"id":"nora","folder":"Campaign","file":"20260901-133258-….png"}'
```

### Drawing a reference sheet

With no photos to start from, JustImagine can draw the character its own references on `google/gemini-3.1-flash-image` (Nano Banana 2):

```sh
curl -s "$BASE/api/characters/refs/generate" -H "content-type: application/json" \
  -d '{"id":"nora","count":5}'
# → {"job":"mtj38lp9-0-7f83","model":"google/gemini-3.1-flash-image","count":5}
```

**The seed matters.** Five independent generations from one description produce five different people, which is useless as an identity reference. So the first shot is drawn from the description and *every other shot is drawn from that first shot* — front, three-quarter, profile, smiling, full-length, each on a plain mid-grey studio background because these are identity references, not finished pictures. A character that already has a reference skips the seed and draws five more angles off what it has.

Watch it on `/api/events` like any other job; its events carry `characterId`, which is how the editor follows it and the gallery ignores it. Shots are written to disk as they land, so a reload picks the set back up.

They arrive as **candidates**, not references — nothing is kept without asking:

```sh
curl -s "$BASE/api/characters/refs/keep" -H "content-type: application/json" \
  -d '{"id":"nora","files":["0c602ae540170a78.png","a51e226811acb282.png"]}'
curl -s "$BASE/api/characters/candidates/clear" -H "content-type: application/json" -d '{"id":"nora"}'
```

A kept shot keeps its filename, so `GET /charref/<file>?c=<id>` serves it either way. Keeping stops at the 8-reference cap rather than silently dropping the overflow, and a shot whose bytes the character already holds is dropped as a duplicate. At five shots this costs roughly 25–30¢.

Then name the cast in a generation:

```sh
curl -s "$BASE/api/generate" -H "content-type: application/json" \
  -d '{"kind":"image","folder":"Campaign","model":"google/gemini-3-pro-image",
       "prompt":"@Nora reading a paperback on a train, window light",
       "characters":["nora"]}'
```

What that does:

- **The references are attached.** For images they join whatever you attached by hand, capped at 8 between them. For video they go to `input_references` — a character portrait is never pinned as frame one, because that would force every clip to open on that exact photo. If you *did* attach a frame by hand it wins, and the response carries a `warning` saying the cast was not sent.
- **The prompt is composed.** `@Nora` becomes plain `Nora` so the sentence reads naturally, and the descriptions are appended after it:

  ```text
  Nora reading a paperback on a train, window light

  The reference images are this character. Keep their face, hair, build and clothing
  consistent with the references:
  - Nora — early 30s, short dark curly hair, freckles, green field jacket
  ```

- **History records both.** The entry keeps the prompt you typed, plus `characters: ["Nora"]`.

An id that names no character is ignored rather than failing the generation. Renaming a character changes its id (the folder moves with it), so re-read `/api/characters` after an update.

> This is reference-driven consistency, not a trained identity. It is the same mechanism as Higgsfield's avatars, not its Soul Characters — good, and better the more references a character has, but not a guarantee.

## Watch jobs

Generation is asynchronous for both kinds, so a five-minute video survives a page reload and no client has to hold a socket open.

```sh
curl -sN "$BASE/api/events"
```

```text
data: {"type":"snapshot","jobs":[…]}

data: {"type":"job","job":{"id":"mtiy3mis-0-99d1","kind":"video","folder":"Campaign",
  "prompt":"…","model":"google/veo-3.1","status":"running","phase":"generating","startedAt":1788284032612}}

data: {"type":"job","job":{"id":"mtiy3mis-0-99d1","status":"done","phase":"","item":{…}}}
```

`status` is `queued`, `running`, `done`, `error` or `cancelled`. `phase` narrates a running job (`submitting`, `queued`, `generating`, `downloading`). A finished job carries the full `item` — the same shape `GET /api/items` returns:

```json
{
  "file": "20260901-133431-a-single-droplet-falling-into-still-water-b62a.mp4",
  "kind": "video",
  "folder": "Campaign",
  "prompt": "a single droplet falling into still water, macro, slow motion",
  "api": "openrouter",
  "model": "x-ai/grok-imagine-video",
  "duration": 1,
  "resolution": "480p",
  "aspectRatio": "16:9",
  "cost": 0.05,
  "generationId": "gen-vid-1788284033-4qzXJP15r4pM5q8VNyJf",
  "bytes": 175171,
  "ms": 38462,
  "ts": 1788284071074
}
```

Jobs are held for a minute after they finish so a reconnecting page still sees the result, then dropped.

At most 8 image and 3 video generations run at once; the rest wait in a queue and report `status: "queued"`.

## Fetch the media

```sh
curl -s "$BASE/media/<file>?f=<folder>" -o out.png       # whole file
curl -s "$BASE/media/<file>?f=<folder>&dl=1" -O          # with a download disposition
curl -s -H "range: bytes=0-1023" "$BASE/media/<file>?f=" # first KB, 206 Partial Content
```

Generated files never change under their name, so they are served `immutable` with a one-year max-age.

Thumbnails are produced by the browser — one canvas draw of media it has already decoded — and posted back to `/api/thumb`, which keeps the grid fast without pulling an image codec into the CLI. A script can seed them the same way; `GET /thumb/…` simply 404s until one exists.

## Errors

Every route answers with `{"error":"…"}` and a non-200 status. A generation that fails upstream is reported through the job feed rather than the HTTP response, since the request returns before the work starts:

```json
{"type":"job","job":{"id":"…","status":"error","error":"402 out of credit"}}
```
