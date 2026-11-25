import React from 'react';
import { Audio, Sequence, staticFile } from 'remotion';
import { FPS } from './theme';
import { scenes } from './timeline';
import { lin } from './ui';

/** Windows (global frames) in which she is speaking. */
const speaking = scenes.flatMap((s) => s.beats.map((b) => ({ from: s.from + b.from + b.begin, to: s.from + b.from + b.end })));

/** 0 when nobody speaks, 1 while she does: ramps in over 7 frames before a word, out over 14 frames after. */
export const voiceness = (g: number): number => {
  let v = 0;
  for (const w of speaking) {
    const inn = lin(g, w.from - 8, 7);
    const out = 1 - lin(g, w.to + 3, 14);
    v = Math.max(v, Math.min(inn, out));
  }
  return v;
};

/** A level that drops to `under` while she speaks. */
export const ducked = (g: number, level: number, under: number): number => level * (1 - (1 - under) * voiceness(g));

export type SongPart = {
  /** Global frame it starts and ends at. */
  from: number;
  to: number;
  track: 'full' | 'bed';
  /** Seconds into the track at `from`. */
  songAt: number;
  /** Level when nobody speaks, and as a share of that while she does. */
  level: number;
  under: number;
  fadeIn?: number;
  fadeOut?: number;
};

/** The song laid in at a place in it, quieter under her voice. */
export const Song: React.FC<SongPart> = ({ from, to, track, songAt, level, under, fadeIn = 6, fadeOut = 8 }) => (
  <Sequence from={from} durationInFrames={to - from} layout="none">
    <Audio
      src={staticFile(`song/${track}.m4a`)}
      startFrom={Math.round(songAt * FPS)}
      volume={(f) => ducked(from + f, level, under) * lin(f, 0, fadeIn) * (1 - lin(f, to - from - fadeOut, fadeOut))}
    />
  </Sequence>
);
