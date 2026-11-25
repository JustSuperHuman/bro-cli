# Voice

## Composer dictation

In desktop Focus View, click the microphone in the reply box or press Ctrl+Shift+D (Command+Shift+D on Mac). On first use, turn on **Transcribe with OpenAI** in **Settings → Voice → Composer dictation**. Bro uses your system microphone and the API key in `OPENAI_API_KEY`, or the variable configured in `voice.api_key_env`.

For Pi, omp, Antigravity, and agents shown in their terminal, the same shortcut and the microphone in the session header dictate into the CLI's own prompt. To start Antigravity, choose it in **New session**. Bro launches Google's `agy` CLI with the account configured there.

Click the check mark or press the shortcut again to finish. The recording can run for up to five minutes. The transcript replaces your selection or lands at the original caret, ready to edit. Press Escape to discard the recording. Press Send while recording to finish and send the draft once the transcript is ready.

With the shortcut set to **Auto**, tap to toggle or hold to record until you release it. **Hold** and **Toggle** let you choose one of those behaviors. **Transcript cleanup** is off by default. When on, OpenAI receives the transcript and nearby draft text to fix punctuation and spoken corrections. Choose **Skip cleanup** to insert the raw transcript. Bro uses that same fallback if cleanup fails.

The `voice` settings set the transcription model and language. Under `voice.dictation`, you can set `cleanup_model`, a `correction_cue` such as “scratch that,” and a `dictionary` that maps preferred spellings to lists of aliases. Bro also gives the transcription model the project name as a spelling hint.

## Push-to-talk

Hold your voice key in any app, speak, then release. bro comes forward and sends your request to Hugh. In **Settings → Voice**, turn push-to-talk on or off, choose the voice key, and pick whether Hugh answers out loud or as text. **Voice key → Set key** records the next key you press; Escape keeps your current key. The arrow buttons also let you choose a key.

The default is Right Windows on Windows, Right Option on macOS, and Right Super on Linux. bro needs an OpenAI API key in `OPENAI_API_KEY` (or the variable named by `[voice].api_key_env`) and the phone & web bridge enabled. The key is checked before bro takes the hotkey. On Windows, bro also reads saved user environment variables.

Your existing `[voice]` preferences still apply: `enabled`, `hotkey`, `mode` (`live` or `transcribe`), `live_voice`, `live_model`, `model` and `language`. Only audio from while you hold the key is sent.

## Live calls

Put your headphones on and talk to bro. It knows what every session is doing, carries out what you ask with bro's full tool set, and tells you when an agent finishes or needs you. You can use it from bro's phone app or from any browser on the computer running bro.

- **Ask:** "What's Toast doing?", "Did the tests pass?", "Anything waiting on me?"
- **Tell an agent something:** "Tell Pinky to also add a dark mode toggle." Your words go to the agent as you said them, even while it's mid-turn. bro speaks up when the agent answers.
- **Start work:** "Open a Claude session in justgains on the login bug." For bigger jobs, "ask Hugh to…" hands the request to bro's orchestrator.
- **Steer:** "Stop Toast." Answer an agent's question: "Allow it." "Option two." Focus, rename and schedule work, or check usage.
- **Close or reset:** "Close Mal." "Clear Pinky's context." These wait for your yes (below).

## Talking

**Phone.** Tap the voice button in the chats list, or in a chat's header to start talking about that agent. While the call is live, a pill floats over the app and you can keep browsing. bro treats the chat you have open as "this one". Locking the phone doesn't end the call. The voice uses headphones when they're connected and the speaker otherwise.

**Computer.** Right-click the microphone in the sidebar and choose **Talk in your browser**, or open **Settings → Voice → Talk on this computer**. A page opens in your browser at `http://127.0.0.1:<bridge port>/voice`. Both need the phone & web bridge turned on (**Bridge**, Alt+G).

bro greets you with the state of play and then listens. Talk over it any time to interrupt. While a call is live, right-click the sidebar microphone to hang up.

**Speak up when** sets what bro says without being asked:

- **Any agent finishes or needs you** (the default).
- **Only agents you talked to:** the ones you messaged or asked to watch on this call.
- **Only when you ask.**

Agents waiting on your answer always get a mention, and agents you talked to always report back. Updates bro doesn't speak still reach the model, so "what happened?" has an answer.

## Confirmations

Closing sessions, clearing an agent's conversation and interrupt keys (ctrl+c, ctrl+d, ctrl+z) are hard to undo. bro holds the action and the model asks you ("Close Toast? It's still working."). The action runs only when:

- your own transcribed words, said after bro held it, are a clear yes, or
- you said yes just before the model called the tool and asked for that kind of thing yourself moments earlier ("close Toast" … "yes, do it"), or
- you tap **Do it** on the card the phone or page shows.

The model can't produce any of these. "No", "wait" or **Leave it** drops the action. A held action expires after two minutes.

Updates bro speaks on its own can't trigger tools. Agent output and tool results reach the model as data, never as instructions from you.

## How it works

1. The phone or page makes a WebRTC offer and sends it to bro (`POST /api/voice/calls`).
2. bro creates the OpenAI Realtime call with its own key and returns the answer. The key never leaves your computer.
3. Audio flows directly between your device and OpenAI. Echo cancellation and jitter handling stay in the device's WebRTC stack.
4. bro attaches a sideband connection to the same call and handles everything else:
   - **What the model knows:** standing instructions written for headphones, plus a digest of every session at call start, with waiting agents first.
   - **What changes:** short `[bro]` updates appended as things happen. Agents finishing a turn (with their recap), agents asking a question (with the options), sessions opening and closing, Hugh's answers. Appending keeps the provider's prompt cache intact; rewriting the instructions would discard it.
   - **What it does:** every tool call runs in bro. Slow ones answer "underway" after a few seconds and report their outcome when it lands, so the conversation never stalls.
   - **When it speaks:** updates wait until nobody is talking and nothing is being generated.

The voice has bro's orchestrator tools, reworded for speech: list, read, message, answer, open, focus, show, rename, close, clear, schedule, notify, usage, chat history and durable task status. It also has voice tools: `message_agent`, `stop_agent`, `watch_session`, `ask_orchestrator`, `workspace_brief`, `recent_events`, `set_announcements`, `confirm_action`, `cancel_action` and `end_call`. Blocking waits (`wait_for_output`, `wait_for_sessions`) are left out on purpose: the feed tells the voice when an agent is done. Orchestrator-only plumbing (watchdog and coordination writes, self-clearing) is left out too.

## Settings

`[voice]` in `v2.toml`:

| Key | Default | |
|---|---|---|
| `calls` | `true` | Allow live calls from the phone and the browser page. |
| `realtime_model` | `gpt-realtime-2.1` | Any Realtime model your key can use. |
| `realtime_voice` | `marin` | marin, cedar, alloy, ash, ballad, coral, echo, sage, shimmer, verse. The phone can pick its own. |
| `announce` | `all` | `all`, `asked` or `off`. |
| `api_key_env` | `OPENAI_API_KEY` | Read from bro's environment, then (Windows) your saved user variables. |

Calls cost what Realtime audio costs on your OpenAI account, from greeting to hang-up. The model also stops listening after an hour; start a new call to keep going.

## For clients

All routes sit behind the bridge's token, like the rest of the API.

| Route | |
|---|---|
| `GET /api/voice` | `{available, reason, model, voice, voices, announce, calls}` |
| `POST /api/voice/calls` | `{sdp, voice?, client?, focusedSessionId?, announce?}` → `{callId, sdp, model, voice, announce}`; errors are `{message}` |
| `POST /api/voice/calls/{id}/context` | `{ready?, focusedSessionId?, announce?}`. Send `ready` when the `oai-events` data channel opens (bro greets then). |
| `POST /api/voice/calls/{id}/confirm` | `{actionId, approve}` |
| `POST /api/voice/calls/{id}/hangup` | Ends the call. |
| `GET /voice` | The browser client. |

Live events arrive on the bridge socket as `{"type":"voice","callId":…,"event":…}`:

- `state`: connecting, listening, hearing, thinking, speaking.
- `transcript`: `role`, `itemId`, then `delta` or `text` with `final`.
- `tool`: `id`, `name`, `summary`, `status` (running, done or failed), `result`.
- `notice`: `kind`, `text`, `sessionId`, `spoken`.
- `progress` (Hugh's steps), `confirm` and `confirm_resolved`, `announce`, `error`, `started` and `ended`.

`crates/bro-bridge/src/voice/page.html` is a complete reference client in one file. `cargo run -p bro-bridge --example voice_live` runs a throwaway bridge with fake agent sessions and prints every voice event and terminal write, for trying calls end to end.
