# Video generation pricing comparison

Verified September 26, 2026. Prices are USD per generated video second, not generation wall-clock time.

## Method

Higgsfield was inspected through its public pricing page and live video generator, without submitting any jobs. Current Plus: $59 monthly / 1,200 credits; $47 per month billed annually / 1,200 monthly credits ($564 annual commitment). Effective unit rates assume all credits are used, allocate the entire subscription price to the credit pool, and exclude Unlimited windows. The chart uses the currently displayed lower promotional quote where two rates appear.

OpenRouter prices came from https://openrouter.ai/api/v1/videos/models, saved alongside this file. Add 5.5% to inference prices to reflect standard credit funding. Taxes and minimum top-up fee effects are excluded. No reference video, paid reference assets, continuation, or upscale is included. Audio enabled except MiniMax H3 Max, which does not support audio generation on OpenRouter. Kling is compared to the OpenRouter Standard 720p route; Higgsfield calls it simply Kling 3.0 and does not separately expose a Standard/Pro name.

Seedance conversion at 16:9, 1280x720, 24 fps: 1280 * 720 * 24 / 1024 = 21,600 tokens per generated second. Seedance 2.0: 21,600 * $7 / 1,000,000 = $0.1512/s before fee. Seedance 2.5: 21,600 * $10.70 / 1,000,000 = $0.23112/s before fee. Source: https://openrouter.ai/bytedance/seedance-2.0 and https://openrouter.ai/bytedance/seedance-2.5.

## Live Higgsfield quotes

| Model | Resolution | Seconds | Current credits | Undiscounted credits shown |
| --- | --- | ---: | ---: | ---: |
| Veo 3.1 Lite | 720p | 8 | 12 | — |
| MiniMax H3 Max | 768p, silent | 5 | 12.5 | — |
| Wan 3.0 | 720p | 5 | 9 | 13 |
| Veo 3.1 Fast | 720p | 8 | 32 | — |
| Kling 3.0 | 720p, audio On | 5 | 10 | — |
| MiniMax H3 | 2K (model's fixed output) | 5 | 10 | 20 |
| Grok Imagine 1.5 | 720p | 5 | 22.5 | — |
| Seedance 2.0 | 720p | 8 | 36 | 48 |
| FLUX.3 Video | 720p | 5 | 27.5 | — |
| Seedance 2.5 | 720p, 16:9 | 5 | 35 | — |
| Veo 3.1 | 720p | 8 | 80 | — |

Quote duration matters: the chart divides each observed quote by its own duration; it does not assert every clip length has identical effective pricing. Seedance 2.0 Fast was inspected (20 credits / 8s at 720p) but omitted because the interface explicitly said unavailable in the US. Sora was not included because a current selectable model quote was not established.

Higgsfield sources: https://higgsfield.ai/ai/video and https://higgsfield.ai/pricing. OpenRouter fee: https://openrouter.ai/pricing. These are published quotes, not paid generation benchmarks or proof of output quality.
