import React from 'react';
import { useCurrentFrame } from 'remotion';
import { songAt } from './clips';
import lyrics from './data/lyrics.json';
import { cutToGator } from './shots';
import { C, FONT, FPS } from './theme';
import { Beat, scenes, sceneOf } from './timeline';
import { lin } from './ui';

type Word = { text: string; t0: number; t1: number };
type Piece = { words: Word[]; t0: number; t1: number };

const MAX = 44;

/** Words (with the share of the line's time each takes) grouped into pieces short enough to read at a glance. */
const group = (words: Word[]): Piece[] => {
  const out: Piece[] = [];
  let cur: Word[] = [];
  const len = (ws: Word[]) => ws.map((w) => w.text).join(' ').length;
  const close = () => {
    if (cur.length) out.push({ words: cur, t0: cur[0].t0, t1: cur[cur.length - 1].t1 });
    cur = [];
  };
  for (const w of words) {
    const prev = cur[cur.length - 1];
    if (cur.length && (len(cur) + 1 + w.text.length > MAX || (/[.?!]$/.test(prev.text) && len(cur) >= 12))) close();
    cur.push(w);
  }
  close();
  return out;
};

/** A narrated line: each word's time is its letters (and a beat after each stop) as a share of the line. */
const pieces = (show: string): Piece[] => {
  const raw = show.split(/\s+/).filter(Boolean);
  const weight = (w: string) => w.replace(/[^\p{L}\p{N}]/gu, '').length + 1.2 + (/[.?!]$/.test(w) ? 3 : /[,:;]$/.test(w) ? 1.6 : 0);
  const total = raw.reduce((n, w) => n + weight(w), 0);
  let seen = 0;
  return group(
    raw.map((text) => {
      const t0 = seen / total;
      seen += weight(text);
      return { text, t0, t1: seen / total };
    }),
  );
};

/**
 * Lines of the film's own song that land on their own, with the narration quiet: captioned like her lines, with the
 * words timed from the song's own word times (scripts/prepare.mjs reads them out of whisper-words.json).
 * `anchor` gives the frame in the scene at which a moment of the song plays.
 */
type SongLine = { key: 'cold' | 'gator' | 'end'; scene: string; anchor: (t: number) => number };
const SONG_LINES: SongLine[] = [
  { key: 'cold', scene: 'cold', anchor: (t) => (t - songAt('cold')) * FPS },
  { key: 'gator', scene: 'film', anchor: (t) => cutToGator(sceneOf('film')) + (t - songAt('gator')) * FPS },
  { key: 'end', scene: 'end', anchor: (t) => (t - 103.1) * FPS },
];
type SungWord = { text: string; t0: number; t1: number };
const songBeats = (sceneId: string): Beat[] =>
  SONG_LINES.filter((l) => l.scene === sceneId).map((l) => {
    const sung = (lyrics as Record<string, SungWord[]>)[l.key] ?? [];
    const first = sung[0]?.t0 ?? 0;
    const last = sung[sung.length - 1]?.t1 ?? first + 1;
    const from = Math.round(l.anchor(first));
    const frames = Math.max(1, Math.round((last - first) * FPS));
    const say = sung.map((w) => w.text).join(' ');
    return { id: `song-${l.key}`, file: null, say, show: say, typed: null, from, frames, slot: frames, begin: 0, end: frames };
  });

const cut = new Map<string, Piece[]>();
for (const scene of scenes) {
  for (const beat of scene.beats) cut.set(beat.id, pieces(beat.show));
  for (const l of SONG_LINES.filter((x) => x.scene === scene.id)) {
    const sung = (lyrics as Record<string, SungWord[]>)[l.key] ?? [];
    if (!sung.length) continue;
    const first = sung[0].t0;
    const span = Math.max(0.01, sung[sung.length - 1].t1 - first);
    cut.set(`song-${l.key}`, group(sung.map((w) => ({ text: w.text, t0: (w.t0 - first) / span, t1: (w.t1 - first) / span }))));
  }
}

/** What is being said, at the bottom of the frame, for watching with the sound off. The word being spoken is pink. */
export const Captions: React.FC = () => {
  const f = useCurrentFrame();
  let found: { beat: Beat; local: number; scene: string } | null = null;
  for (const scene of scenes) {
    for (const beat of [...scene.beats, ...songBeats(scene.id)]) {
      const local = f - scene.from - beat.from;
      if (local >= beat.begin - 4 && local <= beat.end + 10) found = { beat, local, scene: scene.id };
    }
  }
  if (!found) return null;
  const { beat, local, scene: sceneId } = found;
  const span = Math.max(1, beat.end - beat.begin);
  const at = Math.min(0.9999, Math.max(0, (local - beat.begin) / span));
  const list = cut.get(beat.id) ?? [];
  const piece = list.find((p) => at >= p.t0 && at < p.t1) ?? list[list.length - 1];
  if (!piece) return null;
  const last = piece === list[list.length - 1];
  const start = beat.begin + piece.t0 * span;
  const end = last ? beat.end + 10 : beat.begin + piece.t1 * span;
  const opacity = Math.min(lin(local, start - 4, 4), last ? 1 - lin(local, end - 5, 5) : 1);
  return (
    <div style={{ position: 'absolute', left: sceneId === 'end' ? 760 : 0, right: 0, bottom: 52, display: 'flex', justifyContent: 'center', fontFamily: FONT }}>
      <div style={{ padding: '14px 32px 16px', borderRadius: 18, background: C.panel, border: `1.5px solid ${C.line}`, boxShadow: '0 10px 34px rgba(0,0,0,0.5)', opacity, whiteSpace: 'nowrap', fontSize: 44, fontWeight: 600, letterSpacing: '-0.012em', color: C.ink, lineHeight: 1.15 }}>
        {piece.words.map((w, i) => {
          const spoken = at >= w.t0;
          const now = at >= w.t0 && at < w.t1;
          return (
            <span key={i} style={{ color: now ? C.pink : spoken ? C.ink : 'rgba(255,255,255,0.62)', marginRight: i < piece.words.length - 1 ? '0.28em' : 0 }}>
              {w.text}
            </span>
          );
        })}
      </div>
    </div>
  );
};
