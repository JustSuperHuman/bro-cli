import React from 'react';
import { Audio, Easing, Img, Sequence, interpolate, staticFile, useCurrentFrame } from 'remotion';
import { C, FONT, MONO, MX, W } from './theme';
import type { Scene } from './timeline';

export type SceneFC = React.FC<{ scene: Scene }>;

export const ease = Easing.bezier(0.2, 0.7, 0.2, 1);
const clamp = { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' } as const;

/** 0 to 1 over `dur` frames from `start`, eased. */
export const prog = (frame: number, start: number, dur = 16) => interpolate(frame, [start, start + dur], [0, 1], { ...clamp, easing: ease });
/** The same without easing, for things that move at a steady rate. */
export const lin = (frame: number, start: number, dur: number) => interpolate(frame, [start, start + dur], [0, 1], clamp);
export const mix = (a: number, b: number, t: number) => a + (b - a) * t;

/** Fades and lifts its children in at `at`, and out again at `out` when given. Keeps its place in the layout. */
export const Rise: React.FC<{ at: number; out?: number; by?: number; dur?: number; dim?: { at: number; to: number }; style?: React.CSSProperties; children?: React.ReactNode }> = ({ at, out, by = 22, dur = 16, dim, style, children }) => {
  const f = useCurrentFrame();
  const p = prog(f, at, dur);
  const q = out === undefined ? 0 : prog(f, out, 10);
  const d = dim ? mix(1, dim.to, prog(f, dim.at, 14)) : 1;
  return <div style={{ opacity: p * (1 - q) * d, transform: `translateY(${(1 - p) * by - q * 8}px)`, ...style }}>{children}</div>;
};

/** Fades a whole scene in and out so the cut between two scenes is soft. */
export const SceneShell: React.FC<{ frames: number; children: React.ReactNode }> = ({ frames, children }) => {
  const f = useCurrentFrame();
  const opacity = Math.min(lin(f, 0, 9), 1 - lin(f, frames - 9, 9));
  return <div style={{ position: 'absolute', inset: 0, opacity }}>{children}</div>;
};

/** The small label and the title at the top of a scene. Titles swap at their `at` frames. */
export const Header: React.FC<{ eyebrow: string; titles: { at: number; text: string }[]; size?: number }> = ({ eyebrow, titles, size = 60 }) => {
  const f = useCurrentFrame();
  return (
    <div style={{ position: 'absolute', left: MX, top: 78, width: W - MX * 2 }}>
      <div style={{ fontFamily: MONO, fontSize: 21, letterSpacing: '0.14em', textTransform: 'uppercase', color: C.muted, opacity: prog(f, 2, 12) }}>{eyebrow}</div>
      <div style={{ position: 'relative', height: size * 1.2, marginTop: 14 }}>
        {titles.map((t, i) => {
          const next = titles[i + 1]?.at;
          const p = prog(f, t.at, 14);
          const q = next === undefined ? 0 : prog(f, next - 8, 8);
          return (
            <div key={i} style={{ position: 'absolute', left: 0, top: 0, whiteSpace: 'nowrap', fontSize: size, fontWeight: 600, letterSpacing: '-0.025em', lineHeight: 1.15, opacity: p * (1 - q), transform: `translateY(${(1 - p) * 14 - q * 10}px)` }}>
              {t.text}
            </div>
          );
        })}
      </div>
    </div>
  );
};

type Tone = 'plain' | 'raised' | 'accent' | 'ok' | 'warn' | 'danger' | 'blue';
const tones: Record<Tone, { bg: string; line: string; ink: string }> = {
  plain: { bg: C.panel, line: C.line, ink: C.ink },
  raised: { bg: C.panel2, line: C.lineStrong, ink: C.ink },
  accent: { bg: C.accentSoft, line: C.accentLine, ink: C.accent },
  ok: { bg: C.okSoft, line: 'rgba(143,207,156,0.4)', ink: C.ok },
  warn: { bg: C.warnSoft, line: 'rgba(232,194,104,0.4)', ink: C.warn },
  danger: { bg: C.dangerSoft, line: 'rgba(241,126,126,0.42)', ink: C.danger },
  blue: { bg: C.blueSoft, line: 'rgba(127,178,255,0.42)', ink: C.blue },
};

export const Card: React.FC<{ tone?: Tone; pad?: number | string; radius?: number; style?: React.CSSProperties; children?: React.ReactNode }> = ({ tone = 'plain', pad = 32, radius = 22, style, children }) => (
  <div style={{ background: tones[tone].bg, border: `1.5px solid ${tones[tone].line}`, borderRadius: radius, padding: pad, boxSizing: 'border-box', ...style }}>{children}</div>
);

export const Pill: React.FC<{ tone?: Tone; dot?: boolean; size?: number; style?: React.CSSProperties; children: React.ReactNode }> = ({ tone = 'plain', dot, size = 24, style, children }) => (
  <span style={{ display: 'inline-flex', alignItems: 'center', gap: size * 0.42, height: size * 1.85, padding: `0 ${size * 0.8}px`, borderRadius: 999, background: tone === 'plain' ? 'rgba(255,255,255,0.07)' : tones[tone].bg, color: tone === 'plain' ? C.ink2 : tones[tone].ink, fontSize: size, fontWeight: 500, whiteSpace: 'nowrap', ...style }}>
    {dot ? <span style={{ width: size * 0.42, height: size * 0.42, borderRadius: 999, background: 'currentColor' }} /> : null}
    {children}
  </span>
);

export const Label: React.FC<{ style?: React.CSSProperties; children: React.ReactNode }> = ({ style, children }) => (
  <div style={{ fontFamily: MONO, fontSize: 20, letterSpacing: '0.13em', textTransform: 'uppercase', color: C.muted, ...style }}>{children}</div>
);

export const Kbd: React.FC<{ children: React.ReactNode; size?: number }> = ({ children, size = 22 }) => (
  <span style={{ display: 'inline-flex', alignItems: 'center', justifyContent: 'center', minWidth: size * 1.7, height: size * 1.7, padding: `0 ${size * 0.45}px`, borderRadius: 8, border: `1.5px solid ${C.lineStrong}`, background: C.panel2, color: C.ink2, fontFamily: MONO, fontSize: size, boxSizing: 'border-box' }}>{children}</span>
);

const paths: Record<string, React.ReactNode> = {
  laptop: (<><rect x="4" y="5" width="16" height="11" rx="2" /><path d="M2 19.5h20" /></>),
  monitor: (<><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M8 20h8M12 16v4" /></>),
  phone: (<><rect x="7" y="2.5" width="10" height="19" rx="2.5" /><path d="M11 18.5h2" /></>),
  lock: (<><rect x="5" y="11" width="14" height="9" rx="2" /><path d="M8 11V8a4 4 0 0 1 8 0v3" /></>),
  check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  x: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  search: (<><circle cx="11" cy="11" r="6.5" /><path d="M16 16l4.5 4.5" /></>),
  file: (<><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" /><path d="M14 3v5h5" /></>),
  clock: (<><circle cx="12" cy="12" r="8.5" /><path d="M12 7.5V12l3 2" /></>),
  key: (<><rect x="3.5" y="5" width="17" height="14" rx="3" /><path d="M8.5 14.5h7" /></>),
  terminal: <path d="M5 8l4 4-4 4M12 16h7" />,
  alert: (<><path d="M12 4l9 15.5H3z" /><path d="M12 10v4.5M12 17.3v.2" /></>),
  arrow: <path d="M4 12h15M13 6l6 6-6 6" />,
  wait: (<><circle cx="12" cy="12" r="8.5" /><path d="M8 12h.01M12 12h.01M16 12h.01" /></>),
  server: (<><rect x="4" y="4" width="16" height="7" rx="2" /><rect x="4" y="13" width="16" height="7" rx="2" /><path d="M8 7.5h.01M8 16.5h.01" /></>),
  wrench: <path d="M14.5 6.5a4 4 0 0 0 5 5L11 20a2.1 2.1 0 0 1-3-3l8.5-8.5a4 4 0 0 1-2-2z" />,
  dot: <circle cx="12" cy="12" r="3.2" />,
};

/** Line icons on a 24 grid. `knot` is the Just Superhuman mark and `bro` is bro's icon. */
export const Icon: React.FC<{ name: string; size?: number; color?: string; stroke?: number; style?: React.CSSProperties }> = ({ name, size = 28, color = 'currentColor', stroke = 1.8, style }) => {
  if (name === 'bro') return <Img src={staticFile('img/bro.png')} style={{ width: size, height: size, display: 'block', ...style }} />;
  if (name === 'knot') {
    return (
      <svg width={size} height={size} viewBox="0 0 64 64" style={{ display: 'block', flex: 'none', ...style }}>
        <path fill={color} d="M13.12 40.27 6.26 40.27 23.89 9.73 37.43 33.18 40.94 33.18 23.89 3.65 1 43.31 11.36 43.31ZM36.68 15.67 40.11 9.73 57.74 40.27 30.66 40.27 28.91 43.31 63 43.31 40.11 3.65 34.92 12.63ZM46.2 48.37 49.63 54.31 14.37 54.31 27.91 30.86 26.15 27.82 9.11 57.35 54.89 57.35 49.71 48.37Z" />
      </svg>
    );
  }
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={stroke} strokeLinecap="round" strokeLinejoin="round" style={{ display: 'block', flex: 'none', ...style }}>
      {paths[name]}
    </svg>
  );
};

/** A tick or a cross in a small tinted circle. */
export const Mark: React.FC<{ kind: 'ok' | 'fail' | 'skip'; size?: number }> = ({ kind, size = 34 }) => {
  const tone = kind === 'ok' ? { bg: C.okSoft, ink: C.ok } : kind === 'fail' ? { bg: C.dangerSoft, ink: C.danger } : { bg: 'rgba(255,255,255,0.06)', ink: C.faint };
  return (
    <span style={{ width: size, height: size, borderRadius: 999, background: tone.bg, display: 'inline-flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
      <Icon name={kind === 'ok' ? 'check' : kind === 'fail' ? 'x' : 'dot'} size={size * 0.62} color={tone.ink} stroke={2.4} />
    </span>
  );
};

/** A sound from the pack at a frame of the scene it is in. */
export const Sfx: React.FC<{ at: number; name: 'whoosh-short' | 'tick-bright' | 'pop' | 'chime-success' | 'impact-soft'; volume?: number }> = ({ at, name, volume = 0.1 }) => (
  <Sequence from={Math.max(0, Math.round(at))} durationInFrames={90} layout="none">
    <Audio src={staticFile(`sfx/${name}.wav`)} volume={volume} />
  </Sequence>
);

export const mono = (size: number, color: string = C.ink2): React.CSSProperties => ({ fontFamily: MONO, fontSize: size, color, fontVariantLigatures: 'none' });
export const body = (size: number, color: string = C.ink2, weight = 400): React.CSSProperties => ({ fontFamily: FONT, fontSize: size, color, fontWeight: weight, lineHeight: 1.35 });
