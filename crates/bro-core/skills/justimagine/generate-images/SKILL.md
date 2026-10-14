---
name: generate-images
description: Generate images (illustrations, product shots, thumbnails, ads, storyboards, scenes with recurring characters) through the local JustImagine server, including the saved cast of named characters with reference images and style variants like "(Mystery)". Use whenever the user asks to create, draw, render or generate pictures or images, put a cast member / character / "@Name" in a picture, make a character reference sheet, or add a new character. For video use generate-video; for speech use generate-voice.
---

# Generate images with JustImagine

JustImagine is a local gallery server that keeps provider keys server-side and
files every result in folders on disk. Drive it with the helper in this skill's
directory. It finds the server, signs in, starts the service if it is down,
resolves the cast, checks every item against the model, prices the batch,
queues it, waits and prints absolute file paths.

```sh
J="<this skill's directory>/scripts/ji.mjs"
node "$J" up            # where it runs, which providers have keys, how big the cast is
```

If `up` says it can't be started: `bro install justimagine`. If a provider has
no key: the user adds it in the gallery's Settings (`bro imagine` opens it) or
with `bro --env <file> --save`.

## The loop

1. `node "$J" cast [filter]`, only when people or characters are involved.
2. `node "$J" models image [--sort cost|quality|speed|new]`: ready models with price, measured speed and Design Arena rank. Sort by what the user cares about: `quality` for hero shots, `speed` for iteration, `cost` for bulk.
3. Write the spec to a file in your scratch dir, then `node "$J" plan spec.json`. Fix every `errors` entry and read the `warnings`.
4. `node "$J" run spec.json`. It blocks until done and prints `files`, `failed` and `cost`.
5. Open the images you made (read the PNGs) before describing them. Report paths and failures as they are.

Default model: `google/gemini-3.1-flash-image` (about 7¢, best with references).
Higher fidelity: `google/gemini-3-pro-image` (about 13¢). Cheap drafts:
`openai/gpt-5-image-mini` (under 1¢). The catalogue is live, so check `models`
when it matters. Gemini and GPT chat-image models ignore `size` and `quality`:
put the aspect and framing in the prompt ("wide 16:9 banner", "vertical 9:16").

## Spec

Same shape as `POST /api/batch`: `defaults` merge under every item.

```json
{
  "defaults": { "kind": "image", "model": "google/gemini-3.1-flash-image",
                "folder": "Client/Ads", "style": "mystery" },
  "items": [
    "@hector reviews a loan file with @jess, late-night office, warm desk lamp",
    { "prompt": "@carlos shakes hands with a client outside a bank", "count": 3 },
    { "prompt": "put this logo on a tote bag", "images": ["/abs/path/logo.png"], "style": "base" }
  ]
}
```

| Field | Notes |
| --- | --- |
| `prompt` | Required. `@name` pulls a cast member in. |
| `folder` | Always set one, named for the task (`Client/Campaign`). |
| `count` | Copies of the item, at most 12. A batch holds 50 items and 100 jobs. |
| `model`, `api` | From `models image`; `api` fills itself in. |
| `images` | Hand references: absolute paths, or stored names. At least 300px. Earlier outputs work too: use the path `run` printed. |
| `cast` | Cast ids or names, as well as `@mentions`. |
| `style` | Picks the cast's style variant (`base`, `mystery`…). |
| `enhance` | `true` has the server rewrite a thin prompt (about 0.1¢). Leave a prompt the user wrote with care alone. |

Exit codes: 0 done, 2 invalid spec (nothing queued), 3 over the cost cap, 4 some generations failed.

## The cast

The cast is global, the same in every gallery. A character has a name, a
description that often fixes an art style, and up to 8 reference images. One
person can exist in several styles as separate characters (`nora`,
`nora-mystery`). `node "$J" cast` groups people with their `style=` variants,
reference counts and voices; `--full` prints whole descriptions.

- Name them with `@firstname`, `@id` or `"cast": [...]`. The helper swaps the mention for the character's name and sends the id, so the server attaches the references and appends the description.
- Set the style once: `"style": "mystery"` in `defaults` makes every `@name` resolve to its Mystery variant, and `"style": "base"` picks the plain one. An ambiguous name is an error that lists the options. Choose one; don't guess.
- Don't describe their looks in the prompt (hair, face, clothes, art style). The references carry that, and restating it drifts the likeness. Describe the action, setting, framing and light.
- One style per shot. Mixing styles is flagged, and two variants of one person in one item is an error.
- 8 references per image: hand images first, then the cast in order. Three characters with 3 references each already overflow, and the planner names who gets cut. Put the most important character first.
- A character with `refs=0` works from its description alone, which is weak. Give it references first.

### Adding a character

```sh
node "$J" cast-new "Nora" "early 30s, short dark curly hair, freckles, green field jacket. <art style, if she always renders in one>"
node "$J" cast-sheet nora 5      # draws a consistent reference sheet (about 35¢) and prints the candidates
# open each candidate; keep only the ones that match each other and the description
node "$J" cast-keep nora 0c60….png a51e….png
node "$J" cast-ref nora /abs/photos/nora.jpg     # or a real photo, or a generation you liked
```

A new style for an existing person: `cast-new "<Name> (<Style>)"` with the same
physical description plus the style sentence. Render the original in the new
style (`"@nora, redrawn as <style>, plain grey background, head and shoulders"`
with `"style": "base"`), `cast-ref` the best result into the variant, then
`cast-sheet` it. The sheet is drawn from that seed, so it keeps the likeness and
the new style. Seeding it with the original's references would copy the old style.

## Money and care

- `run` refuses estimates over `--max-cost 3` (USD). Tell the user the estimate from `plan` and get a yes before re-running with `--yes` and a higher cap.
- One batch for a whole set; the server runs 8 images at once. Don't loop `run` per prompt.
- Never delete folders, generations or characters to tidy up unless asked.
- Server trouble: `node "$J" up` says what it found. The service's own log: `node <JustImagine>/bin/justimagine.js service logs`.
