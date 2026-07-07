# Image Generation HTTP API

`bro image` starts a local web server for the image generation UI. The UI does not use any private browser-only path: it calls JSON routes on that same server. You can call those routes from scripts as long as the `bro image` process is running.

The server binds to `127.0.0.1` and does not add a separate API auth layer. Keep it local. Your upstream image API key stays server-side, loaded from `~/.bro/config.json` or the provider's environment variable.

## Start The Server

```sh
bro image -p yunwu
# or
bro image -p openai
```

The command prints the actual base URL, output directory, and context directory:

```text
UI:      http://127.0.0.1:8790
Output:  <cwd>/.bro/image-gen
Context: <cwd>/.bro/context
```

Use the printed URL as `BASE` in the examples below.

## Generate An Image

```sh
curl -s "$BASE/api/generate" \
  -H "content-type: application/json" \
  -d '{
    "prompt": "a clean product photo of a steel water bottle",
    "model": "gpt-image-2",
    "size": "1024x1024",
    "quality": "high"
  }'
```

Response:

```json
{
  "file": "20260705-143205-a-clean-product-photo-of-a-steel-water-bottle-k8p2.png",
  "prompt": "a clean product photo of a steel water bottle",
  "model": "gpt-image-2",
  "size": "1024x1024",
  "quality": "high",
  "ms": 18420,
  "ts": 1783276325000
}
```

Fetch the image with:

```sh
curl -L "$BASE/images/20260705-143205-a-clean-product-photo-of-a-steel-water-bottle-k8p2.png" -o image.png
```

## Request Body

`POST /api/generate`

```json
{
  "prompt": "required text prompt",
  "model": "optional model id; defaults to the API's first configured model",
  "size": "auto, 1024x1024, 1536x1024, 1024x1536, etc.",
  "quality": "auto, low, medium, or high",
  "images": ["optional-context-file.png"]
}
```

Notes:

- `prompt` is required.
- `model` can be any configured model or custom model id.
- `size` and `quality` are omitted upstream when set to `auto`.
- Chat-routed image models, such as Gemini flash image models, ignore `size` and `quality`; describe those requirements in the prompt.
- The route generates one image per request. For batches, issue multiple concurrent `POST /api/generate` requests, which is what the web UI does.

## Reference Images

Reference images are stored in the context library before generation. This mirrors paste, drag/drop, and attach behavior in the web UI.

1. Convert the image to a base64 image data URL.
2. Upload it to `/api/context`.
3. Pass the returned `file` name in the `images` array for `/api/generate`.

```sh
curl -s "$BASE/api/context" \
  -H "content-type: application/json" \
  -d '{"dataUrl":"data:image/png;base64,iVBORw0KGgo..."}'
```

Response:

```json
{
  "file": "9f3a1b2c4d5e6f70.png",
  "existed": false
}
```

Then generate with that reference:

```sh
curl -s "$BASE/api/generate" \
  -H "content-type: application/json" \
  -d '{
    "prompt": "use the reference image and make a studio product shot",
    "model": "gpt-image-2",
    "size": "1024x1024",
    "quality": "high",
    "images": ["9f3a1b2c4d5e6f70.png"]
  }'
```

Up to 8 context file names are accepted. For image-API models, references are sent upstream through the edits endpoint. For chat-routed image models, references are sent as vision input.

## Routes

| Method | Route | Purpose |
| --- | --- | --- |
| `GET` | `/` | Serves the browser UI. |
| `GET` | `/api/state` | Returns active API info, models, output directory, generation history, and context library entries. |
| `POST` | `/api/generate` | Generates one image, saves it to `.bro/image-gen`, appends history, and returns metadata. |
| `POST` | `/api/context` | Saves a base64 image data URL into `.bro/context` by content hash. |
| `POST` | `/api/context/delete` | Deletes one context image by file name. |
| `GET` | `/images/<file>` | Serves a generated image file. |
| `GET` | `/context/<file>` | Serves a context/reference image file. |
| `POST` | `/api/delete` | Deletes one generated image by file name. |
| `POST` | `/api/delete-all` | Deletes all generated image files and the history file. Context images are kept. |

## Errors

Most API errors return JSON with an `error` message:

```json
{
  "error": "Prompt is required."
}
```

Common status codes:

- `400` for invalid client input, such as a missing prompt or invalid context data URL.
- `404` for missing image/context files or unknown routes. These currently return plain text.
- `500` when the upstream image provider fails or returns an unsupported image payload.

## State And History

`GET /api/state` is useful for clients that want to present the same model picker and gallery as the web UI:

```sh
curl -s "$BASE/api/state"
```

The `history` entries are loaded from `.bro/image-gen/history.jsonl`, and only entries whose files still exist are returned. Generated images are written to `.bro/image-gen`; context images are written to `.bro/context`.
