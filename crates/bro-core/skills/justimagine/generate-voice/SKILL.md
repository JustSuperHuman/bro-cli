---
name: generate-voice
description: Generate speech through the local JustImagine server on ElevenLabs (voice-overs, narration, ad reads, character lines, and dialogue scenes performed as one take with each saved cast member in their own voice), and design, save or assign a character's voice. Use whenever the user asks for a voice-over, narration, text to speech, a character saying or reading something, dialogue or scene audio, or to give a character a voice. For images use generate-images; for video use generate-video.
---

# Generate voice with JustImagine

JustImagine is a local gallery server that keeps the ElevenLabs key
server-side and files every voice line (MP3) in folders on disk. Saved cast
members can have their own voice. Drive it with the helper in this skill's
directory. It finds the server, signs in, starts the service if it is down,
checks every voice and speaker before anything is spent, queues, waits and
prints absolute file paths.

```sh
J="<this skill's directory>/scripts/ji.mjs"
node "$J" up            # "elevenlabs (no key)" means the user has to add one first
```

If `up` says it can't be started: `bro install justimagine`. No ElevenLabs key:
the user adds it in the gallery's Settings (`bro imagine` opens it), or sets
`ELEVENLABS_API_KEY` with `bro --env <file> --save`, which restarts the service.

## The loop

1. `node "$J" cast` to see who has a voice (`voice=`), or `node "$J" voices [filter]` for the account's voices (`premade`, `cloned`, `generated`, labelled by gender, age, accent and use).
2. `node "$J" models voice` when the model matters (below).
3. Write the spec to a scratch file, then `node "$J" plan spec.json`. Fix `errors` (a missing voice, a speaker without one, text over the model's limit) and read `warnings`.
4. `node "$J" run spec.json`. It prints `files`, `failed` and the estimate in ElevenLabs credits.
5. Report the paths. You can't hear the result, so don't describe how it sounds; say what was read and by whom.

## Models

| Model | Use |
| --- | --- |
| `eleven_v3` (default) | The most expressive. Audio tags like `[whispers]`, `[laughs]`, `[sighs]`, and the only model for multi-speaker scenes. |
| `eleven_multilingual_v2` | Steady narration in 29 languages: long reads, audiobooks, explainers. |
| `eleven_flash_v2_5`, `eleven_turbo_v2_5` | Half the credits, fast. Drafts and bulk lines. |

Credits are per character; `models voice` shows each model's multiplier and
character limit. A long script is several items, not one.

## Spec

```json
{
  "defaults": { "kind": "voice", "folder": "Client/VO", "model": "eleven_v3" },
  "items": [
    { "prompt": "[warmly] Welcome back. Today we fix the one thing slowing your mornings down.",
      "voice": "21m00Tcm4TlvDq8ikWAM" },
    { "prompt": "The door creaked open.\nNora: [whispers] Where were you?\nSam: Out.\nNora: All night?",
      "voice": "21m00Tcm4TlvDq8ikWAM" },
    { "prompt": "I told you the numbers would hold.", "cast": ["sam"], "count": 2 }
  ]
}
```

| Field | Notes |
| --- | --- |
| `prompt` | The words. On `eleven_v3`, audio tags in brackets direct the delivery and aren't spoken. |
| `voice` | A voice id from `voices`. In a scene it is the narrator: lines before the first speaker. |
| `cast` | A cast member who has a voice reads the whole line in it. |
| `folder` | Always set one, named for the task. |
| `count` | Takes of the same line, at most 4. Two or three takes of an important line give the user a choice. |
| `stability`, `similarity`, `style` | 0–1. On `eleven_v3`, stability has three steps: 0 creative, 0.5 natural, 1 robust. Unset means the voice's own settings. |
| `speed` | 0.7–1.2. |
| `language` | `"en"`, `"ja"`… for the multilingual models. |
| `format` | `mp3_44100_128` (default), `mp3_44100_192`, `mp3_44100_64`. |
| `enhance` | `true` has the server add pacing punctuation and, on models that perform them, audio tags. Leave a script the user wrote with care alone. |

### Scenes

A line that starts with a cast member's name (`Nora: …`) is theirs, in their
voice; only cast names count, so "Note: bring coffee" stays text. A scene with
several speakers is performed as one take on `eleven_v3`. Every speaker needs a
voice; the planner names anyone who doesn't have one.

## Giving a character a voice

```sh
# three candidates from a description (at least 20 characters: age, accent, pitch, timbre, pace)
node "$J" voice-design "a warm, husky alto in her thirties, soft Dublin accent, unhurried" --text "a line she would say"
# → candidate files; the user listens and picks one
node "$J" voice-save <generatedVoiceId> nora
# or give a character a voice already on the account
node "$J" voice-assign nora <voiceId>
```

Let the user choose between the candidates: play the files or hand them the
paths. Don't pick for them. Base the description on the character's
description when they don't give one. Cloning from recordings happens in the
gallery's Cast tab, and only for voices the user has the right to use.

## Care

- ElevenLabs bills credits from the user's plan. Mention the credit estimate for anything longer than a few lines.
- One batch per set. The server runs as many voice jobs as the plan allows and queues the rest; `waiting for a free ElevenLabs slot` isn't a failure.
- `failed` entries carry the reason (no credits, a voice that was deleted, text too long). Report them as they are.
