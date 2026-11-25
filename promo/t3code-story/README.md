# What bro took from T3 Code

A narrated Remotion story (1920×1080, 30 fps, about three and a half minutes) on what changed in bro 2.1.49 after
reading T3 Code's source, and what to expect next. The long form is `reports/t3code-deep-dive/index.html`.

```sh
npm install
npm run prepare-assets     # screenshots, sounds, music bed, timing, numbers from ~/.bro/v2-perf.log
npm run studio             # preview
npm run render             # out/bro-t3code-story.mp4, sound levelled to about -16 LUFS
npm run check              # does each spoken line start where the timeline puts it?
```

`public/`, `out/` and the two generated files in `src/data/` are git-ignored. `prepare-assets` rebuilds all of them
except the voice-over, which costs ElevenLabs credits (about 3,100 for the whole script).

## How it is put together

- `scripts/beats.json` is the script: one entry per spoken line, with the scene it belongs to. `say` is what the
  voice reads; `show` is the caption when it differs (figures instead of spelled-out numbers); `min` keeps a line
  on screen longer than it takes to say, for an animation that needs the time.
- `scripts/prepare.mjs` measures each voice file and writes `src/data/timing.json`. `src/timeline.ts` lays the
  scenes out from that, so re-recording a line moves everything after it by itself.
- Scenes ask for the frame a phrase is spoken at with `cue(beat, 'phrase')` (`src/timeline.ts`), so what appears
  stays with the words. A phrase that isn't in the line fails loudly.
- `src/data/perf.json` holds two sets of numbers: the reading of the log from before 2.1.49 (fixed, in
  `prepare.mjs`), and what 2.1.49 has logged since, read fresh on every `prepare-assets`.

## Re-recording the voice

```sh
node scripts/vo-spec.mjs spec.json            # every line; add beat ids to redo only those
node ~/.claude/skills/generate-voice/scripts/ji.mjs run spec.json > run.txt
node scripts/prepare.mjs --vo run.txt         # whole script only: it matches files to lines in order
```

For a single line, copy the new file over `public/vo/<id>.mp3` and run `prepare-assets`.

## Checking frames

```sh
node scripts/stills.mjs --table                  # where each scene and line starts
node scripts/stills.mjs --beats                  # the middle and end of every line
node scripts/stills.mjs --frames 840,1110        # these frames
python scripts/sheet.py out/stills out/sheets 2 960 3
```
