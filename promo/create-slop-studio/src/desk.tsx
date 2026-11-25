import React, { createContext, useContext } from 'react';
import { Audio, Img, Sequence, interpolate, staticFile, useCurrentFrame } from 'remotion';
import { C, H, W } from './theme';
import { lin, mix, prog, smooth } from './ui';

/** A box in the capture's own pixels: x, y, width, height. */
export type Box = [number, number, number, number];

/** The desktop picture desk-shot.py makes with --margin 45 is the capture plus 45 px of wallpaper all round. */
export const OFF = 45;
export const PIC_W = 1850;
export const PIC_H = 1040;
/** Screen pixels per picture pixel when the whole picture fills the frame. */
export const BASE = W / PIC_W;

/** A desktop picture's size, and the scale at which it covers the frame. Windows of another shape get their own. */
export type Pic = { w: number; h: number; base: number };
export const picOf = (w: number, h: number): Pic => ({ w, h, base: Math.max(W / w, H / h) });
export const DEFAULT_PIC: Pic = picOf(PIC_W, PIC_H);
/** The 1440x930 windows (the toolkit capture and the storyboard close-ups) with 45 px of desktop all round. */
export const SMALL_PIC: Pic = picOf(1530, 1020);

export type Cam = { cx: number; cy: number; z: number };
const fullOf = (pic: Pic): Cam => ({ cx: pic.w / 2, cy: pic.h / 2, z: 1 });

/** Where on screen (0 to 1 of the height) the middle of a pushed-in box sits: above the captions. */
const BOX_Y = 0.43;

/** The camera that frames `box` (capture px): as large as fits in 88% of the width and 64% of the height, never past `zmax`. */
export const frame = (box: Box | null, zmax = 2.6, pic: Pic = DEFAULT_PIC): Cam => {
  if (!box) return fullOf(pic);
  const [x, y, w, h] = box;
  const fit = Math.min((W * 0.88) / (w * pic.base), (H * 0.64) / (h * pic.base));
  const z = Math.max(1, Math.min(zmax, fit));
  const k = pic.base * z;
  const halfW = W / 2 / k;
  const halfH = H / 2 / k;
  const cx = Math.min(pic.w - halfW, Math.max(halfW, x + OFF + w / 2));
  const cy = Math.min(pic.h - halfH, Math.max(halfH, y + OFF + h / 2 + (0.5 - BOX_Y) * (H / k)));
  return { cx, cy, z };
};

/** The view that makes `box` fill the whole frame exactly (for the pull-back from a full-frame clip). */
export const exact = (box: Box, pic: Pic = DEFAULT_PIC): Cam => {
  const [x, y, w] = box;
  const k = W / w;
  return { cx: x + OFF + w / 2, cy: y + OFF + (w * (H / W)) / 2, z: k / pic.base };
};

export type Move = { at: number; box: Box | null; zmax?: number; dur?: number };

/** Frames a move takes at least, so a push-in is never faster than it can be read. */
const minFrames = (a: Cam, b: Cam, base: number) => {
  const dz = Math.abs(Math.log(b.z / a.z));
  const dist = Math.hypot(b.cx - a.cx, b.cy - a.cy) * base;
  return Math.round(28 + 40 * dz + 14 * (dist / 1000));
};

export const camAt = (moves: Move[], f: number, drift = 0.012, total = 300, pic: Pic = DEFAULT_PIC): Cam => {
  let cur = fullOf(pic);
  for (const m of moves) {
    if (f < m.at) break;
    const tgt = frame(m.box, m.zmax, pic);
    // a move at frame 0 or earlier is where the shot starts, not a move
    if (m.at <= 0) { cur = tgt; continue; }
    const dur = Math.max(m.dur ?? 0, minFrames(cur, tgt, pic.base));
    const p = smooth(f, m.at, dur);
    const z = Math.exp(mix(Math.log(cur.z), Math.log(tgt.z), p));
    const next = { cx: mix(cur.cx, tgt.cx, p), cy: mix(cur.cy, tgt.cy, p), z };
    if (p < 1) { cur = next; break; }
    cur = tgt;
  }
  const z = cur.z * (1 + drift * lin(f, 0, total));
  return { ...cur, z };
};

type CamValue = { k: number };
const CamContext = createContext<CamValue>({ k: BASE });
/** Screen pixels per picture pixel right now: things drawn in the picture divide by this to keep a constant size on screen. */
export const useK = () => useContext(CamContext).k;

/** The transform that puts the camera's view on the 1920x1080 frame. */
export const worldStyle = (cam: Cam, pic: Pic = DEFAULT_PIC): React.CSSProperties => {
  const k = pic.base * cam.z;
  return { position: 'absolute', left: 0, top: 0, width: pic.w, height: pic.h, transformOrigin: '0 0', transform: `translate(${W / 2 - cam.cx * k}px, ${H / 2 - cam.cy * k}px) scale(${k})` };
};

export type Layer = { src: string; at: number; fade?: number; out?: number };

const Layers: React.FC<{ layers: Layer[]; pic: Pic }> = ({ layers, pic }) => {
  const f = useCurrentFrame();
  const opac = layers.map((l, i) => {
    const fadeIn = i === 0 && l.at <= 0 ? 1 : lin(f, l.at, l.fade ?? 8);
    const fadeOut = l.out === undefined ? 0 : lin(f, l.out, l.fade ?? 8);
    return fadeIn * (1 - fadeOut);
  });
  // only what can be seen is drawn: nothing that hasn't started, nothing hidden under a later picture that is fully in
  let cover = -1;
  opac.forEach((o, i) => { if (o >= 1) cover = i; });
  return (
    <>
      {layers.map((l, i) => {
        if (opac[i] <= 0 || i < cover) return null;
        return <Img key={`${l.src}-${i}`} src={staticFile(`desk/${l.src}.png`)} style={{ position: 'absolute', left: 0, top: 0, width: pic.w, height: pic.h, opacity: opac[i] }} />;
      })}
    </>
  );
};

/**
 * A capture on the desktop, full frame, with a camera that pushes into boxes given in the capture's own pixels.
 * `layers` swap one picture for the next (cross-fade); `overlay` is drawn in picture coordinates, so it moves with the camera.
 */
export const Desk: React.FC<{ layers: Layer[]; moves?: Move[]; total: number; drift?: number; pic?: Pic; overlay?: React.ReactNode }> = ({ layers, moves = [], total, drift = 0.012, pic = DEFAULT_PIC, overlay }) => {
  const f = useCurrentFrame();
  const cam = camAt(moves, f, drift, total, pic);
  return (
    <CamContext.Provider value={{ k: pic.base * cam.z }}>
      <div style={worldStyle(cam, pic)}>
        <Layers layers={layers} pic={pic} />
        {overlay}
      </div>
    </CamContext.Provider>
  );
};

// ---- pointer -----------------------------------------------------------------------------------------------

/** Where the pointer arrives (capture px) at frame `at`; `click` presses there. */
export type Pt = { at: number; x: number; y: number; click?: boolean };

const travel = (a: { x: number; y: number }, b: { x: number; y: number }) => Math.min(54, Math.max(20, 16 + Math.hypot(b.x - a.x, b.y - a.y) * BASE * 0.045));

const ARROW = 'M2 2 L2 30 L9.6 23.4 L14.6 35 L20.2 32.6 L15.2 21.4 L25 21.4 Z';

/** A drawn pointer: glides between points, presses and rings pink on a click. Constant size on screen at any zoom. */
export const Pointer: React.FC<{ pts: Pt[]; size?: number; sound?: boolean }> = ({ pts, size = 1, sound = true }) => {
  const f = useCurrentFrame();
  const k = useK();
  if (!pts.length) return null;
  const first = pts[0];
  // position: between the point just left and the next one
  let pos = { x: first.x + 150, y: first.y + 110 };
  let from = pos;
  let heading = 0;
  for (let i = 0; i < pts.length; i++) {
    const to = pts[i];
    const t0 = Math.max(0, to.at - travel(from, to));
    if (f < to.at) {
      const p = smooth(f, t0, to.at - t0);
      const dx = to.x - from.x;
      const dy = to.y - from.y;
      const arc = Math.sin(Math.PI * p) * 0.07;
      pos = { x: mix(from.x, to.x, p) - dy * arc, y: mix(from.y, to.y, p) + dx * arc };
      heading = i;
      break;
    }
    pos = { x: to.x, y: to.y };
    from = pos;
    heading = i;
  }
  const show = Math.min(lin(f, first.at - 40, 10), 1);
  // a click: the last click within 7 frames presses; rings expand for 22
  const clicks = pts.filter((p) => p.click && f >= p.at - 1 && f < p.at + 24);
  const press = pts.some((p) => p.click && f >= p.at && f < p.at + 6) ? 0.86 : 1;
  const s = (size * 1.15) / k;
  void heading;
  return (
    <>
      {clicks.map((c) => {
        const p = prog(f, c.at, 22);
        const r = (10 + 46 * p) / k;
        return (
          <div key={c.at} style={{ position: 'absolute', left: c.x + OFF - r, top: c.y + OFF - r, width: r * 2, height: r * 2, borderRadius: 999, border: `${3 / k}px solid ${C.pink}`, background: `rgba(255,95,168,${0.2 * (1 - p)})`, opacity: 1 - p * p, boxSizing: 'border-box' }} />
        );
      })}
      <div style={{ position: 'absolute', left: pos.x + OFF, top: pos.y + OFF, width: 0, height: 0, opacity: show, transform: `scale(${s * press})`, transformOrigin: '0 0' }}>
        <svg width="30" height="38" viewBox="0 0 30 38" style={{ position: 'absolute', left: -2, top: -2, overflow: 'visible', filter: 'drop-shadow(0 2px 3px rgba(0,0,0,0.55))' }}>
          <path d={ARROW} fill="#ffffff" stroke="#0b0d11" strokeWidth="2.4" strokeLinejoin="round" />
        </svg>
      </div>
      {sound && pts.filter((p) => p.click).map((p) => (
        <Sequence key={`s${p.at}`} from={Math.max(0, p.at)} durationInFrames={20}>
          <Audio src={staticFile('sfx/tick-bright.wav')} volume={0.5} />
        </Sequence>
      ))}
    </>
  );
};

/** A pink full border around a box (capture px), fading in at `at` and out at `out`. Constant line width at any zoom. */
export const Ring: React.FC<{ box: Box; at: number; out?: number; pad?: number; radius?: number }> = ({ box, at, out, pad = 6, radius = 10 }) => {
  const f = useCurrentFrame();
  const k = useK();
  const o = lin(f, at, 8) * (out === undefined ? 1 : 1 - lin(f, out, 8));
  if (o <= 0) return null;
  const [x, y, w, h] = box;
  const grow = 1 + 0.012 * (1 - prog(f, at, 14));
  return (
    <div style={{ position: 'absolute', left: x + OFF - pad, top: y + OFF - pad, width: w + pad * 2, height: h + pad * 2, borderRadius: radius, border: `${3 / k}px solid ${C.pink}`, boxShadow: `0 0 ${18 / k}px ${C.pinkGlow}`, opacity: o, boxSizing: 'border-box', transform: `scale(${grow})` }} />
  );
};

/** A caret that blinks at a point (capture px). */
export const Caret: React.FC<{ x: number; y: number; h?: number; at: number }> = ({ x, y, h = 18, at }) => {
  const f = useCurrentFrame();
  const k = useK();
  if (f < at) return null;
  const on = Math.floor((f - at) / 15) % 2 === 0;
  return <div style={{ position: 'absolute', left: x + OFF, top: y + OFF, width: 2.2 / k, height: h, background: C.pink, opacity: on ? 1 : 0 }} />;
};

/** Plays a number between 0 and 1 from frame `a` to `b`, for sliding things. */
export const useSpan = (a: number, b: number) => {
  const f = useCurrentFrame();
  return interpolate(f, [a, b], [0, 1], { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' });
};
