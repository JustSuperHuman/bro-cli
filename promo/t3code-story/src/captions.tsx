import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C } from './theme';
import { Beat, scenes } from './timeline';
import { body, lin } from './ui';

type Piece = { text: string; from: number; to: number };

const clausesOf = (text: string) => text.split(/(?<=[.,:;?!])\s+/);

/**
 * A line cut into pieces short enough to read at a glance, each with the share of the line's time it covers.
 * The caption writes numbers as figures ("2,405") where the voice spells them out, so a piece's share comes
 * from how long its words take to say, not from how wide it is.
 */
const pieces = (show: string, say: string): Piece[] => {
  const MAX = 66;
  const clauses = clausesOf(show);
  const spoken = clausesOf(say);
  const cost = clauses.map((clause, i) => (spoken.length === clauses.length ? spoken[i].length : clause.length));
  const lines: { text: string; weight: number }[] = [];
  clauses.forEach((clause, k) => {
    const last = lines[lines.length - 1];
    if (last && last.text.length + 1 + clause.length <= MAX && !/[.?!]$/.test(last.text)) {
      last.text = `${last.text} ${clause}`;
      last.weight += cost[k];
      return;
    }
    if (clause.length <= MAX) {
      lines.push({ text: clause, weight: cost[k] });
      return;
    }
    // a clause too long for one piece breaks at spaces, into pieces of about the same width
    const target = clause.length / Math.ceil(clause.length / MAX);
    let line = '';
    const made: string[] = [];
    for (const word of clause.split(' ')) {
      if (line && line.length + 1 + word.length > target + 6) {
        made.push(line);
        line = word;
      } else {
        line = line ? `${line} ${word}` : word;
      }
    }
    if (line) made.push(line);
    for (const part of made) lines.push({ text: part, weight: cost[k] * (part.length / clause.length) });
  });
  const total = lines.reduce((n, l) => n + l.weight, 0);
  let seen = 0;
  return lines.map((l) => {
    const from = seen / total;
    seen += l.weight;
    return { text: l.text, from, to: seen / total };
  });
};

const cut = new Map<string, Piece[]>();
for (const scene of scenes) for (const beat of scene.list) cut.set(beat.id, pieces(beat.show, beat.say));

/** What is being said, at the bottom of the frame, for watching with the sound off. */
export const Captions: React.FC = () => {
  const f = useCurrentFrame();
  let found: { beat: Beat; local: number } | null = null;
  for (const scene of scenes) {
    for (const beat of scene.list) {
      const local = f - scene.from - beat.from;
      if (local >= beat.begin - 3 && local <= beat.end + 8) found = { beat, local };
    }
  }
  if (!found) return null;
  const { beat, local } = found;
  const span = beat.end - beat.begin;
  const at = Math.min(0.999, Math.max(0, (local - beat.begin) / span));
  const list = cut.get(beat.id) ?? [];
  const piece = list.find((p) => at >= p.from && at < p.to) ?? list[list.length - 1];
  if (!piece) return null;
  const last = piece.to >= 0.999;
  const start = beat.begin + piece.from * span;
  const end = last ? beat.end + 8 : beat.begin + piece.to * span;
  const opacity = Math.min(lin(local, start - 3, 4), last ? 1 - lin(local, end - 4, 4) : 1);
  return (
    <div style={{ position: 'absolute', left: 0, right: 0, bottom: 46, display: 'flex', justifyContent: 'center' }}>
      <div style={{ ...body(31, C.ink, 500), letterSpacing: '-0.005em', padding: '10px 26px', borderRadius: 14, background: 'rgba(11,13,17,0.78)', opacity, whiteSpace: 'nowrap' }}>{piece.text}</div>
    </div>
  );
};
