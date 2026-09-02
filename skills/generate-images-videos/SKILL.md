---
name: generate-images-videos
description: Generate images and video in bulk through JustImagine, the local gallery server that ships with bro (`bro imagine`). Covers starting the server, choosing a model, queueing many generations in one call, waiting for them, attaching reference images, reusing characters, and collecting the finished files. Use whenever the task is to create or generate images, pictures, illustrations, thumbnails, product shots, videos, clips, b-roll, or a storyboard.
---

# Generating images and video with JustImagine

JustImagine is a local HTTP server with a JSON API. Everything the browser UI can
do, you can do — including things the UI has no button for, like queueing fifty
prompts in one request and blocking until they finish. Your upstream API keys stay
server-side; you never handle them.

The whole loop is three calls: **pick a model**, **POST /api/batch**, **wait**.

## 0. Reach the server

```sh
BASE=http://127.0.0.1:8791                       # the background service
curl -sf $BASE/api/state >/dev/null && echo up
```

Nothing there? In order of preference:

```sh
bro imagine open --no-open          # prints the URL of whatever is running
bro imagine service install         # background, starts again at every login
bro imagine service status          # installed? reachable? which gallery?
```

- `bro imagine service install` uses the gallery of the **current directory**
  (`./.bro/justimagine`) on port **8791**. Add `--root <dir>` / `--port <n>` to
  change either. Run elevated (`sudo`, or an Administrator terminal) and it starts
  with the machine instead of at login.
- A foreground `bro imagine` serves port **8790** and stops with Ctrl-C. Prefer
  the service: your jobs survive the terminal.
- Loopback only, no auth. Reads are open; writes must not carry another site's
  `Origin` — curl sends none, so scripts are fine.

Set `BASE` once and reuse it. Every path below is relative to it.

## 1. Choose a model

```sh
curl -s "$BASE/api/models?kind=image&detail=1" | jq '.models[] | {id, ready, pricing}'
curl -s "$BASE/api/models?kind=video"          | jq '.models[] | {id, durations, resolutions, aspectRatios, audio, frames}'
curl -s "$BASE/api/models?q=veo"               # search id and name
```

Fields that matter:

| Field | Use it for |
| --- | --- |
| `ready` | `false` means that provider has no key — generating will fail. `GET /api/config` says which providers are configured; the user adds keys in the gallery's Settings panel or in `~/.bro/config.json`. |
| `pricing.perImage` / `perSecond` | Estimated cost. Multiply before you queue a big batch. |
| `durations`, `resolutions`, `aspectRatios`, `sizes` | The only values that model accepts. Sending anything else is an error from the upstream, minutes later. |
| `audio`, `seed`, `frames` | Whether `audio: true`, `seed: n` and first/last-frame conditioning are supported. `frames: ["first_frame"]` means image-to-video works. |

Defaults if you don't care: `google/gemini-3.1-flash-image` for images (fast,
cheap, excellent with reference images), `google/veo-3.1` for video. Video always
runs on OpenRouter, whatever the image API is.

## 2. Queue the work

One spec, `count` copies of it:

```sh
curl -s "$BASE/api/generate" -H 'content-type: application/json' -d '{
  "kind": "image", "folder": "Product/Bottles",
  "model": "google/gemini-3.1-flash-image",
  "prompt": "a steel water bottle on wet slate, side light, product photo",
  "count": 4
}'
# → {"jobs":["mtiy3mis-0-99d1", …], "kind":"image", "model":"…", "folder":"Product/Bottles"}
```

Many *different* prompts, one request — this is the one to reach for:

```sh
curl -s "$BASE/api/batch" -H 'content-type: application/json' -d '{
  "defaults": { "kind": "image", "folder": "Campaign/Boards",
                "model": "google/gemini-3.1-flash-image", "size": "1024x1024" },
  "items": [
    "a cyclist at dawn on a wet city street, shot from behind",
    "the same cyclist locking up outside a cafe, warm interior light",
    { "prompt": "a slow push-in on the cafe window, rain on the glass",
      "kind": "video", "model": "google/veo-3.1", "duration": 8, "audio": true }
  ]
}'
# → {"jobs":[…3 ids…], "count":3, "images":2, "videos":1}
```

- `defaults` is merged **under** every item, so state the folder, model and knobs
  once and let an item override what it needs.
- Images and video mix freely; each item's `kind` picks the queue.
- A bare JSON array of prompt strings is a valid body too.
- The batch is validated whole before anything is queued: one bad item rejects
  the request (`items[7]: Prompt is required.`) and costs nothing.

### Every knob

| Field | Kind | Notes |
| --- | --- | --- |
| `prompt` | both | Required. |
| `kind` | both | `"image"` (default) or `"video"`. |
| `folder` | both | `/`-joined relative path, created on demand. `""` is the top level. Use folders — that is how a gallery stays navigable. |
| `count` | both | Copies of this spec. Max 12 images, 4 videos. |
| `model` | both | From `/api/models`. Defaults to the API's first model. |
| `api` | image | Which image provider. Defaults to the one the server started with. |
| `size`, `quality` | image | For OpenAI-shaped APIs. **Ignored by chat-routed models** (Gemini, GPT-5 Image) — steer those in the prompt. |
| `duration` | video | Seconds. Must be in that model's `durations`. |
| `resolution`, `aspectRatio` | video | Must be in the model's lists. |
| `size` | video | `"1920x1080"`, interchangeable with resolution+aspectRatio; if sent, it wins and the other two are dropped. |
| `audio` | video | Only where `audio: true`. |
| `seed` | video | Only where `seed: true`. Same seed + same prompt ≈ same clip. |
| `images` | both | Reference file names (see §4). |
| `characters` | both | Character ids (see §5). |
| `firstFrame` | video | Default `true`: the first reference becomes frame one (image-to-video) when the model supports it. Set `false` to send every reference as style guidance instead. |
| `lastFrame` | video | Needs a second reference and a model whose `frames` include `last_frame`. |
| `wait` | both | Seconds to block for (see §3). |

## 3. Wait for it

Generation is asynchronous — the POST returns as soon as the jobs are queued.
Two ways to collect them.

**Block on the request** (best for images, which take seconds):

```sh
curl -s "$BASE/api/batch" -H 'content-type: application/json' -d '{
  "defaults": {"folder":"Set","model":"google/gemini-3.1-flash-image"},
  "items": ["a red door", "a blue door"], "wait": 120
}' | jq '{settled, items: [.items[].file], failed}'
```

**Poll** (best for video, which takes minutes):

```sh
IDS=$(curl -s "$BASE/api/batch" -H 'content-type: application/json' \
      -d @batch.json | jq -r '.jobs | join(",")')

# Each call blocks up to 60s, returning early the moment everything lands.
until curl -s "$BASE/api/jobs?ids=$IDS&wait=60" | jq -e '.settled' >/dev/null; do
  curl -s "$BASE/api/jobs?ids=$IDS" | jq -r '.results[] | "\(.status)\t\(.phase)\t\(.prompt[0:40])"'
done
curl -s "$BASE/api/jobs?ids=$IDS" | jq '.items'
```

The reply to either:

```json
{ "settled": true,
  "items":  [ { "file": "20260901-…-a-red-door-b62a.png", "folder": "Set", "kind": "image",
                "model": "…", "bytes": 1512094, "ms": 4210, "cost": 0.04, "ts": 1788284071074 } ],
  "failed": [ { "id": "…", "prompt": "…", "model": "…", "error": "402 out of credit" } ],
  "cancelled": [], "pending": [], "results": [ …full job rows… ] }
```

- `settled: false` means the clock ran out, not that anything failed — `pending`
  lists what is still going. Poll again with the same ids.
- `wait` is capped at 600 seconds per request. A long video needs several rounds.
- `status` is `queued`, `running`, `done`, `error` or `cancelled`; `phase`
  narrates a running job (`submitting`, `queued`, `generating`, `downloading`).
- **A failed generation is reported in `failed`, not as an HTTP error.** Always
  read it; report failures to the user rather than pretending a batch succeeded.
- Finished jobs stay readable for 30 minutes, so a poll can be minutes late.
  `GET /api/jobs` with no ids lists everything still retained; `missing` names ids
  that have aged out.
- `GET /api/events` is the same information as a live server-sent-event stream.
  Prefer polling in a shell; the stream is there if you are writing a UI.

Stop things: `POST /api/cancel` with `{"id":"…"}`, `{"ids":[…]}`, or
`{"all":true}` (optionally `{"all":true,"kind":"video"}`).

## 4. Reference images

The short way: put the path straight into `images`. Anything with a path
separator is registered as it goes past, deduped by content hash, and replaced
with its stored name.

```sh
-d '{"kind":"image","prompt":"put this logo on a canvas tote","images":["/photos/logo.png"]}'
```

The long way, when you want the name up front (to reuse across many items, or to
check it landed):

```sh
FILE=$(curl -s "$BASE/api/context" -H 'content-type: application/json' \
       -d "{\"path\":\"$PWD/logo.png\"}" | jq -r .file)     # a file on disk
# or  -d '{"dataUrl":"data:image/png;base64,iVBORw0…"}'      # bytes inline
```

Either way the same picture is stored once and keeps one name. Then:

```sh
# image editing / style transfer — up to 8 references
-d '{"kind":"image","prompt":"put this logo on a canvas tote","images":["9f1c….png"]}'

# image-to-video: the reference becomes frame one
-d '{"kind":"video","model":"bytedance/seedance-2.0","prompt":"the camera pushes in slowly",
     "images":["9f1c….png"],"firstFrame":true}'
```

- References must be at least 300px on a side, and images (png/jpg/webp/gif).
- An output you just made is a valid input — chain a still into a clip by passing
  `images: ["<root>/<folder>/<file>"]` on the next call. `file` and `folder` come
  from the finished job, `root` from `GET /api/state`.

## 5. Characters — the same person in every shot

```sh
curl -s "$BASE/api/characters"                      # the cast, with ids
curl -s "$BASE/api/characters" -H 'content-type: application/json' \
  -d '{"name":"Nora","description":"early 30s, short dark curly hair, freckles, green field jacket"}'
curl -s "$BASE/api/characters/refs" -H 'content-type: application/json' \
  -d '{"id":"nora","path":"/photos/nora-1.jpg"}'    # or {dataUrl}, or {folder,file}
                                                    # to promote a generation you liked
```

No photos to start from? `POST /api/characters/refs/generate` `{"id":"nora","count":5}`
draws a reference sheet (one portrait from the description, then four angles drawn
*from that portrait*, which is what makes them one person). It returns a job id;
the shots arrive as **candidates** and are kept only when you say so:

```sh
curl -s "$BASE/api/characters/refs/keep" -H 'content-type: application/json' \
  -d '{"id":"nora","files":["0c60….png","a51e….png"]}'
curl -s "$BASE/api/characters/candidates/clear" -H 'content-type: application/json' -d '{"id":"nora"}'
```

Then name the cast in a generation — `@Nora` in the prompt reads naturally and
their references and descriptions are attached automatically:

```sh
-d '{"kind":"image","prompt":"@Nora reading on a train, window light","characters":["nora"]}'
```

For video the references guide the shot (`input_references`) rather than pinning
frame one. This is reference-driven likeness, not a trained identity: good, and
better the more references a character has. The cast is global
(`~/.bro/justimagine/characters`), shared by every gallery.

## 6. Collect the results

```sh
ROOT=$(curl -s "$BASE/api/state" | jq -r .root)      # the gallery on disk
curl -s "$BASE/media/<file>?f=<folder>" -o out.png   # or over HTTP
curl -s "$BASE/api/items?folder=Campaign/Boards&limit=50" | jq '.items[].file'
```

Files land at `<root>/<folder>/<file>` with a `history.jsonl` beside them holding
each generation's prompt, model and settings. Read them straight off disk when you
are on the same machine — that is the fast path. Everything else you might need:
`POST /api/folder`, `/api/folder/rename`, `/api/folder/delete`, `/api/move`,
`/api/delete`. `docs/justimagine-api.md` in the bro repo is the full reference.

## 7. Better prompts, for free

```sh
curl -s "$BASE/api/enhance" -H 'content-type: application/json' \
  -d '{"kind":"video","model":"google/veo-3.1","prompt":"rain on a window"}' | jq -r .prompt
```

Rewrites one line into something the picked model can work with — composition and
light for an image, one named camera move for a clip, bounded by that model's real
clip length. Takes a second or two, costs a fraction of a cent, and reliably
improves output. Worth doing for a batch of thin prompts; skip it when the user
wrote a careful prompt themselves — theirs is the intent, not yours to rewrite.

## 8. Rules of thumb

- **Money is being spent.** Images are ~2–15¢ each; video is priced per second
  and a handful of clips can be several dollars. Compute the cost from
  `/api/models` and **tell the user the number before queueing anything large**.
  Never "explore" with video the way you would with images.
- **Batch, don't loop.** One `/api/batch` for the whole set, then one wait. The
  server already runs 8 images and 3 videos concurrently; a shell loop just makes
  it slower and harder to track.
- **Caps:** 50 items and 100 jobs per batch; 12 images or 4 videos per item.
  Split bigger work into successive batches.
- **Put things in folders**, named for the task. One flat gallery is unusable
  after a week.
- **Send only knobs the model lists.** Check `durations`/`resolutions` first; a
  rejected combination costs you the whole round trip.
- **Report honestly.** Say how many landed, name what failed and why, and give
  the user the paths or the gallery URL. Never describe a picture you have not
  confirmed exists.
- **Deleting a folder deletes every generation in it.** Never delete a folder to
  "clean up" unless the user asked.
