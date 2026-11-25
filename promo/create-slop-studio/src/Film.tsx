import React from 'react';
import { AbsoluteFill, Audio, Sequence, staticFile } from 'remotion';
import { Captions } from './captions';
import { songAt } from './clips';
import { Song } from './mix';
import { Muse, MuseProps } from './muse';
import { Approve, Ask, Board, Cast, Cold, End, Film as FilmScene, Keyframes, Montage, PAUSE_AT, Pull, SceneFC, Start, Tools, cutToGator } from './shots';
import { C, FONT, FPS } from './theme';
import assets from './data/assets.json';
import { cue, sceneOf, scenes } from './timeline';

const SCENES: Record<string, SceneFC> = { cold: Cold, pull: Pull, cast: Cast, tools: Tools, start: Start, board: Board, approve: Approve, ask: Ask, keyframes: Keyframes, film: FilmScene, montage: Montage, end: End };

type Pose = 'neutral' | 'smirk' | 'point';

/**
 * Where Meta Muse stands: small, in a corner with a margin, on the side away from what the shot is pushed in on.
 * Frames are global. She is not in the cold open, the alligator take, the montage (she is in the footage) or
 * at the end card (which has her big).
 */
const museSegments = (): MuseProps[] => {
  const S = sceneOf;
  const out: MuseProps[] = [];
  const g = (id: string, line: string, phrase: string) => S(id).from + cue(S(id), line, phrase);
  const add = (id: string, corner: MuseProps['corner'], poses: [number, Pose][], opts: Partial<MuseProps> = {}, from?: number, to?: number) => {
    const s = S(id);
    out.push({ at: from ?? s.from, out: to ?? s.from + s.frames, corner, poses, ...opts });
  };
  // she arrives once the pull-back has landed
  add('pull', 'bl', [[S('pull').from + 70, 'smirk']], {}, S('pull').from + 70);
  add('cast', 'br', [[S('cast').from, 'neutral'], [g('cast', 'b04', 'Chinese'), 'point'], [g('cast', 'b04', 'Claudia') + 20, 'neutral']], { enter: false });
  add('tools', 'bl', [[S('tools').from, 'neutral'], [g('tools', 'b05b', 'Missing'), 'smirk'], [g('tools', 'b05c', 'No key'), 'neutral']], { enter: false });
  add('start', 'bl', [[S('start').from, 'smirk'], [g('start', 'b06', 'The agent'), 'point'], [g('start', 'b06', 'The film') + 30, 'neutral']], { enter: false });
  add('board', 'br', [[S('board').from, 'neutral'], [g('board', 'n06', 'and what you see'), 'smirk']], { enter: false });
  add('approve', 'bl', [[S('approve').from, 'smirk']], { enter: false });
  add('ask', 'br', [[S('ask').from, 'smirk'], [g('ask', 'b09b', 'Send it'), 'point']], { enter: false });
  add('keyframes', 'br', [[S('keyframes').from, 'neutral'], [g('keyframes', 'n09', 'You pick'), 'point']], { enter: false });
  const film = S('film');
  add('film', 'bl', [[film.from, 'smirk']], { enter: false }, film.from, film.from + cutToGator(film) - 14);
  return out;
};

/** The sound: the song laid in at its place, quieter under her voice (see mix.tsx), and the narration, one file per line. */
const Sound: React.FC = () => {
  const S = sceneOf;
  const cold = S('cold');
  const pull = S('pull');
  const film = S('film');
  const montage = S('montage');
  const end = S('end');
  const cut = film.from + cutToGator(film);
  const pauseFrames = Math.round((PAUSE_AT - songAt('cold')) * FPS);
  return (
    <>
      {/* the cold open: the song as it sits in the film, up to her "Pause." and then nothing */}
      <Song from={cold.from} to={cold.from + pauseFrames} track="full" songAt={songAt('cold')} level={1} under={1} fadeIn={1} fadeOut={3} />
      {/* the song with its voices taken out, under the app shots */}
      <Song from={pull.from} to={cut + 8} track="bed" songAt={29.7} level={0.42} under={0.5} fadeIn={20} fadeOut={8} />
      <Song from={cut} to={film.from + film.frames} track="full" songAt={songAt('gator')} level={1} under={0.3} fadeIn={6} fadeOut={4} />
      {/* the montage: two choruses of the film, the song at the same times as the picture */}
      {(assets.clips.montage.segments as { ss: number; t: number }[]).reduce<React.ReactNode[]>((acc, seg, i, all) => {
        const from = montage.from + all.slice(0, i).reduce((n, g) => n + Math.round(g.t * FPS), 0);
        acc.push(<Song key={i} from={from} to={from + Math.round(seg.t * FPS) + (i === all.length - 1 ? montage.frames - Math.round(all.reduce((n, g) => n + g.t, 0) * FPS) : 0)} track="full" songAt={seg.ss} level={0.95} under={0.3} fadeIn={i === 0 ? 4 : 2} fadeOut={i === all.length - 1 ? 5 : 2} />);
        return acc;
      }, [])}
      {/* the end card: the song's own "You can stop watching now." lands last, then it stops */}
      <Song from={end.from} to={end.from + Math.round((107.65 - 103.1) * FPS)} track="full" songAt={103.1} level={1} under={1} fadeIn={4} fadeOut={5} />
      {scenes.flatMap((s) =>
        s.beats.map((b) =>
          b.file ? (
            <Sequence key={b.id} from={s.from + b.from} durationInFrames={b.frames + 6}>
              <Audio src={staticFile(b.file)} volume={1} />
            </Sequence>
          ) : null,
        ),
      )}
    </>
  );
};

export const Film: React.FC = () => (
  <AbsoluteFill style={{ background: '#000', fontFamily: FONT, color: C.ink, WebkitFontSmoothing: 'antialiased' }}>
    {scenes.map((scene) => {
      const Scene = SCENES[scene.id];
      return (
        <Sequence key={scene.id} name={scene.id} from={scene.from} durationInFrames={scene.frames}>
          <AbsoluteFill>
            <Scene scene={scene} />
          </AbsoluteFill>
        </Sequence>
      );
    })}
    {museSegments().map((m, i) => (
      <Muse key={i} {...m} />
    ))}
    <Captions />
    <Sound />
  </AbsoluteFill>
);
