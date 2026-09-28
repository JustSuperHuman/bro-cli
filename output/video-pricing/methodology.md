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

## OpenLux (added September 28, 2026)

Chart: `video-cost-3way.html` compares Higgsfield Plus at 100% / 75% / 50% of monthly credits used against OpenRouter and OpenLux, and marks the cheapest and second-cheapest option per model.

OpenLux catalogue from https://api.openlux.ai/api/pricing, saved as `openlux-pricing.json`. OpenLux sells $1 of credit for $1 (`price: 1` in `/api/status`). Their docs (doc.openlux.ai, "分组的特殊性及价格差异") state that a route's group ratio multiplies the official price: at ratio 1.65, $1 of official price is charged $1.65.

| Model | OpenLux model / route | Ratio | $/s |
| --- | --- | ---: | ---: |
| Veo 3.1 Lite | not offered | — | — |
| MiniMax H3 Max | aigc-video-hailuo / Aigc-Video-1 | 0.9 | 0.08 × 0.9 = 0.072 |
| Wan 3.0 | wan3.0-video / Alibaba-video-2 | 0.7 | 0.01 × 10 (720p) × 0.7 = 0.070 |
| Veo 3.1 Fast | veo_3_1-fast / Discounted-Gemini-1 | 0.07353 | 0.576 per 8s clip × ratio ÷ 8 = 0.0053 |
| Kling 3.0 | kling-video / Kling-2 | 0.85 | 0.126 × 0.85 = 0.107 |
| MiniMax H3 | aigc-video-hailuo / Aigc-Video-1 | 0.9 | 0.13 × 0.9 = 0.117 |
| Grok Imagine 1.5 | grok-imagine-video-1.5-preview / Xai-Grok-1 | 0.44118 | 0.14 × 0.44118 = 0.062 |
| Seedance 2.0 | doubao-seedance-2-0-260128 / Seedance-1 | 0.65 | 0.1512 × 0.65 = 0.098 |
| FLUX.3 Video | not offered | — | — |
| Seedance 2.5 | doubao-seedance-2-5-260628 / Seedance-1 | 0.65 | 0.23112 × 0.65 = 0.150 |
| Veo 3.1 | veo_3_1 / Discounted-Gemini-1 | 0.07353 | 0.768 per 8s clip × ratio ÷ 8 = 0.0071 |

Wan 3.0 and both Veo rows use prices from OpenLux's own catalogue. For the rest the catalogue lists only a placeholder price (billing is dynamic, based on the upstream price), so the official list rate is taken from OpenRouter's pre-fee price and the route multiplier is applied. The Veo route is an unofficial "discounted" relay with a fixed price per clip; its reliability wasn't verified. OpenLux bulk top-up discounts (1–7.5%) and card fees are excluded.
