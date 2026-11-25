import React from 'react';
import { Img, staticFile, useCurrentFrame } from 'remotion';
import { C } from './theme';
import { lin, prog } from './ui';

export type Pose = 'neutral' | 'smirk' | 'point';
/** The renders are 920x744 after the crop. */
const AR = 744 / 920;

export type MuseProps = {
  /** Frame she flickers on at, and the frame she goes at (omit to stay). */
  at: number;
  out?: number;
  corner?: 'bl' | 'br' | 'tl' | 'tr' | 'center';
  width?: number;
  /** Poses over time: [frame, pose]. The first is where she starts. */
  poses?: [number, Pose][];
  /** Her arm points to the right in the render; flip it to point left. */
  flip?: boolean;
  /** Flicker and rise on at `at`; false for a continuation after a cut. */
  enter?: boolean;
};

/**
 * META MUSE as a small presenter: a hologram of hot-pink light with a slow bob, scan lines drifting down her,
 * a bright sweep, and a halo that pulses. Nothing about her is drawn from scratch; the picture is the render
 * made for the film (public/muse/*.png, from scripts/muse.py).
 */
export const Muse: React.FC<MuseProps> = ({ at, out, corner = 'bl', width = 300, poses = [[0, 'neutral']], flip = false, enter: doEnter = true }) => {
  const f = useCurrentFrame();
  const local = f - at;
  if (local < -1 || (out !== undefined && f > out + 12)) return null;
  const height = width * AR;
  const enter = doEnter ? prog(f, at, 16) : 1;
  const leave = out === undefined ? 0 : lin(f, out, 10);
  // switching on: a few quick flickers, as a projection does
  const flicker = doEnter && local < 9 ? (local % 3 === 1 ? 0.35 : 1) : 1;
  const t = f / 30;
  const bob = Math.sin(t * Math.PI * 2 * 0.42) * 6;
  const sway = Math.sin(t * Math.PI * 2 * 0.21) * 0.6;
  const pulse = 0.5 + 0.5 * Math.sin(t * Math.PI * 2 * 0.8);
  const scan = (f * 0.9) % 12;
  const sweep = ((t * 0.38) % 1) * (height + 200) - 100;
  const fadeTo = (name: Pose) => {
    // cross-fade 6 frames at each change of pose
    let v = 0;
    poses.forEach(([from, p], i) => {
      const start = from;
      const end = poses[i + 1]?.[0] ?? Infinity;
      if (p !== name) return;
      const inn = i === 0 ? 1 : lin(f, start, 6);
      const outt = end === Infinity ? 0 : lin(f, end, 6);
      v = Math.max(v, inn * (1 - outt));
    });
    return v;
  };
  const M = width > 500 ? 0 : 30;
  const B = width > 500 ? -10 : 26;
  const place: React.CSSProperties =
    corner === 'bl' ? { left: M, bottom: B } : corner === 'br' ? { right: M, bottom: B } : corner === 'tl' ? { left: M, top: M } : corner === 'tr' ? { right: M, top: M } : { left: '50%', marginLeft: -width / 2, bottom: B };
  const names: Pose[] = ['neutral', 'smirk', 'point'];
  return (
    <div style={{ position: 'absolute', ...place, width, height, opacity: enter * (1 - leave) * flicker, transform: `translateY(${bob + (1 - enter) * 40}px) rotate(${sway}deg) scaleX(${flip ? -1 : 1})`, filter: `drop-shadow(0 0 22px ${C.pinkGlow}) brightness(${1 + pulse * 0.06})`, WebkitMaskImage: 'linear-gradient(180deg, #000 78%, transparent 100%)' }}>
      {names.map((n) => {
        const o = fadeTo(n);
        return o > 0.001 ? <Img key={n} src={staticFile(`muse/${n}.png`)} style={{ position: 'absolute', inset: 0, width, height, opacity: o }} /> : null;
      })}
      {/* scan lines drifting down her, only where she is (the picture's own alpha is the mask) */}
      {names.map((n) => {
        const o = fadeTo(n);
        return o > 0.001 ? (
          <div key={`s${n}`} style={{ position: 'absolute', inset: 0, opacity: o, WebkitMaskImage: `url(${staticFile(`muse/${n}.png`)})`, WebkitMaskSize: '100% 100%', background: `repeating-linear-gradient(180deg, rgba(40,0,22,0.34) 0px, rgba(40,0,22,0.34) 2px, transparent 2px, transparent 6px)`, backgroundPosition: `0 ${scan}px` }} />
        ) : null;
      })}
      {/* a soft bright band sweeping down */}
      {names.map((n) => {
        const o = fadeTo(n);
        return o > 0.001 ? (
          <div key={`w${n}`} style={{ position: 'absolute', inset: 0, opacity: o * 0.55, WebkitMaskImage: `url(${staticFile(`muse/${n}.png`)})`, WebkitMaskSize: '100% 100%', background: `linear-gradient(180deg, transparent ${sweep - 70}px, rgba(255,235,245,0.75) ${sweep}px, transparent ${sweep + 70}px)` }} />
        ) : null;
      })}
      {/* the halo, brighter on each pulse: the top of the picture drawn again, lit */}
      {names.map((n) => {
        const o = fadeTo(n);
        return o > 0.001 ? (
          <Img key={`h${n}`} src={staticFile(`muse/${n}.png`)} style={{ position: 'absolute', inset: 0, width, height, opacity: o * (0.15 + pulse * 0.85), clipPath: 'inset(0 0 89% 0)', filter: 'brightness(1.7) drop-shadow(0 0 10px rgba(255,255,255,0.9))' }} />
        ) : null;
      })}
    </div>
  );
};
