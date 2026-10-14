---
name: generate-video
description: Generate video clips (b-roll, ads, reels, animated scenes, image-to-video, clips starring saved cast characters) through the local JustImagine server with Veo, Seedance, Kling, Wan, Sora, Hailuo, Grok and more via OpenRouter. Keeps a cast character's likeness by drawing a keyframe first. Use whenever the user asks to make, generate, animate or render a video, clip, shot, reel, b-roll or animation, or to animate an existing image. For stills use generate-images; for speech use generate-voice.
---

# Generate video with JustImagine

JustImagine is a local gallery server that keeps provider keys server-side and
files every clip in folders on disk. Video runs on OpenRouter (and OpenLux when
connected). Drive it with the helper in this skill's directory. It finds the
server, signs in, starts the service if it is down, resolves the cast, checks
every setting against the model, prices the batch, draws keyframes, queues,
polls and prints absolute file paths.

```sh
J="<this skill's directory>/scripts/ji.mjs"
node "$J" up
```

If `up` says it can't be started: `bro install justimagine`. No OpenRouter key:
the user adds it in the gallery's Settings (`bro imagine` opens it) or with
`bro --env <file> --save`.

## The loop

1. `node "$J" models video [query] [--sort cost|speed|quality|new]`: ready models with price, measured time, Design Arena rank, and the only `dur`, `res` and `ar` values each one accepts, plus frame and audio support. `--sort speed` when the user is waiting, `--sort quality` for finals.
2. `node "$J" cast [filter]` if characters appear.
3. Write the spec to a scratch file, then `node "$J" plan spec.json`. Fix `errors`, read `warnings`.
4. Tell the user the estimate and get a go-ahead. Video is priced per second and adds up fast; `run` refuses over `--max-cost 3` without `--yes`.
5. `node "$J" run spec.json [--max-cost N --yes]`. It waits (25 minutes by default) and prints `files`, `keyframes`, `failed` and `cost`. On a timeout it prints a `resume` command (`node "$J" wait <ids>`); jobs keep running and stay collectable for 30 minutes.
6. Check the result before describing it: `ffmpeg -ss 2 -i clip.mp4 -frames:v 1 frame.png`, then open the frame.

## Choosing a model

| Need | Model | About |
| --- | --- | --- |
| Drafts | `bytedance/seedance-1-5-pro` (480p–1080p, 4–12s, audio) | 2.6¢/s |
| Good default | `google/veo-3.1-fast` (4/6/8s, 16:9 or 9:16, audio) | 8¢/s |
| Cheapest Veo | `google/veo-3.1-lite` | 3¢/s |
| Top quality | `google/veo-3.1`, `bytedance/seedance-2.0`, `kwaivgi/kling-v3.0-pro` | 11–20¢/s |
| Long takes (to 30s) | `alibaba/wan-3.0`, `bytedance/seedance-2.5` | 10–23¢/s |

Prices are at 720p; higher resolutions cost more. The catalogue is live, so
confirm with `models video`.

## Spec

```json
{
  "defaults": { "kind": "video", "model": "google/veo-3.1-fast", "folder": "Client/Reels",
                "duration": 8, "resolution": "720p", "aspectRatio": "9:16",
                "audio": true, "style": "mystery" },
  "items": [
    { "prompt": "@hector looks up from a loan file and smiles, slow push-in", "keyframe": true },
    { "prompt": "rain on an office window at night, rack focus to the city", "count": 2 },
    { "prompt": "the camera orbits the product", "images": ["/abs/shots/bottle.png"] }
  ]
}
```

| Field | Notes |
| --- | --- |
| `duration`, `resolution`, `aspectRatio` | Only values the model lists. The planner rejects anything else, which upstream would fail minutes later. Or `size: "1920x1080"`. |
| `audio`, `seed` | Only where the model supports them. |
| `images` | Hand references. With a model that takes `first_frame`, the first image is frame one (image-to-video). `lastFrame: true` with a second image pins the last frame. `firstFrame: false` sends them as style guidance instead. |
| `count` | At most 4 per item. |
| `keyframe` | For cast shots, below. Options: `keyframeModel` (default `google/gemini-3.1-flash-image`), `keyframePrompt` (when the opening still differs from the motion), `keyframeImages` (hand references for the still). |
| `enhance` | `true` has the server rewrite a thin prompt into one camera move that fits the clip. |

Prompt like a shot list: one continuous shot, one named camera move (push-in,
dolly left, orbit, static), and what changes across it. A 4–8 second clip holds
one beat. Describe sound only when `audio` is on.

## Cast in video: use `keyframe: true`

Characters (`@hector`, `"cast": [...]`, `"style": "mystery"`, resolved as in
generate-images) reach a video two ways:

- `keyframe: true`, the one to use. The helper first draws the opening still with the cast's references attached (an image model holds a likeness far better than a video model), then animates it as frame one. The still lands in `<folder>/keyframes/` and costs about 7¢. Needs a model with `first_frame`. Hand references for the still go in `keyframeImages`, not `images`.
- Without it, the references go as `input_references` guidance, which many video models barely weigh, so the likeness drifts. If the item also has a hand image as frame one, the server drops the cast entirely. The planner warns about both.

The stills rules carry over: one `style` per shot, never two variants of one
person, don't restate their looks, and a character with `refs=0` needs
references first (`node "$J" cast-sheet <id>`).

For a sequence with one character across shots, make all the keyframes in one
`run` (one item per shot, each `keyframe: true`) so cast, style and folder stay
consistent. To animate a still the user already approved, skip `keyframe` and
pass its path in `images`; it becomes frame one.

For a character who speaks on camera, make the line with generate-voice first;
the voice and the clip are separate files.

## Care

- Never explore with video the way you would with images: draft on the cheapest model at 480p and short durations, then re-run the chosen prompt on the good model.
- One batch per set; the server runs 3 videos at once and queues the rest.
- `failed` entries carry the upstream error (credit, moderation, a bad frame). Report them; don't hide them.
- Server trouble: `node "$J" up`, then `node <JustImagine>/bin/justimagine.js service logs`.
