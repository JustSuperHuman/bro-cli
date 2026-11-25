import React from 'react';
import { AbsoluteFill, Audio, Sequence, interpolate, staticFile, useCurrentFrame } from 'remotion';
import { Captions } from './captions';
import { Doctor } from './scenes/Doctor';
import { End } from './scenes/End';
import { Intro } from './scenes/Intro';
import { Log } from './scenes/Log';
import { Next } from './scenes/Next';
import { Path } from './scenes/Path';
import { Pool } from './scenes/Pool';
import { Small } from './scenes/Small';
import { Stall } from './scenes/Stall';
import { View } from './scenes/View';
import { C, FONT, MONO, MX } from './theme';
import { CHAPTERS, TOTAL, chapterOf, scenes } from './timeline';
import { SceneFC, SceneShell, prog } from './ui';

const SCENES: Record<string, SceneFC> = { intro: Intro, path: Path, pool: Pool, doctor: Doctor, view: View, log: Log, stall: Stall, small: Small, next: Next, end: End };

/** Which part of the story this is: the four parts in the corner, the current one lit. */
const Chapters: React.FC = () => {
  const f = useCurrentFrame();
  const scene = [...scenes].reverse().find((s) => f >= s.from) ?? scenes[0];
  const current = chapterOf(scene.id);
  const shown = scene.id === 'intro' ? 0 : scene.id === 'end' ? 1 - prog(f, scene.from, 10) : prog(f, scenes[1].from, 14);
  return (
    <div style={{ position: 'absolute', right: MX, top: 80, display: 'flex', gap: 30, alignItems: 'center', opacity: shown }}>
      {CHAPTERS.map((name, i) => (
        <div key={name} style={{ display: 'flex', alignItems: 'center', gap: 10, fontFamily: MONO, fontSize: 19, letterSpacing: '0.1em', textTransform: 'uppercase', color: i === current ? C.ink : C.faint }}>
          <span style={{ width: 9, height: 9, borderRadius: 999, background: i === current ? C.accent : i < current ? C.muted : 'rgba(255,255,255,0.14)' }} />
          {name}
        </div>
      ))}
    </div>
  );
};

export const Story: React.FC = () => {
  return (
    <AbsoluteFill style={{ background: C.bg, fontFamily: FONT, color: C.ink, WebkitFontSmoothing: 'antialiased' }}>
      <AbsoluteFill style={{ background: 'radial-gradient(1300px 760px at 12% -8%, rgba(246,160,77,0.07), transparent 62%), radial-gradient(1100px 760px at 104% 108%, rgba(127,178,255,0.06), transparent 60%)' }} />

      {scenes.map((scene) => {
        const Scene = SCENES[scene.id];
        return (
          <Sequence key={scene.id} name={scene.id} from={scene.from} durationInFrames={scene.frames}>
            <SceneShell frames={scene.frames}>
              <Scene scene={scene} />
            </SceneShell>
          </Sequence>
        );
      })}

      <Chapters />
      <Captions />

      {/* the narration, one file per line */}
      {scenes.flatMap((scene) =>
        scene.list.map((beat) =>
          beat.file ? (
            <Sequence key={beat.id} name={beat.id} from={scene.from + beat.from} durationInFrames={beat.frames + 6} layout="none">
              <Audio src={staticFile(beat.file)} volume={1} />
            </Sequence>
          ) : null,
        ),
      )}

      {/* a quiet bed under it, and a soft sound where one part hands over to the next */}
      <Audio src={staticFile('music/pad.wav')} loop volume={(f) => 0.17 * interpolate(f, [0, 45, TOTAL - 75, TOTAL - 5], [0, 1, 1, 0], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' })} />
      {scenes.slice(1).map((scene) => (
        <Sequence key={scene.id} from={scene.from - 5} durationInFrames={60} layout="none">
          <Audio src={staticFile('sfx/whoosh-short.wav')} volume={0.06} />
        </Sequence>
      ))}
    </AbsoluteFill>
  );
};
