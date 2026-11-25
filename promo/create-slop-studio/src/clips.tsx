import React from 'react';
import { AbsoluteFill, Freeze, Img, OffthreadVideo, Sequence, staticFile, useCurrentFrame } from 'remotion';
import assets from './data/assets.json';
import { Box, BASE, Cam, Layer, OFF, PIC_H, PIC_W, exact, worldStyle } from './desk';
import { FPS, H, W } from './theme';
import { mix, smooth } from './ui';

export type ClipName = keyof typeof assets.clips;

/** Seconds a prepared clip lasts (0 when it hasn't been prepared). */
export const clipSeconds = (name: string): number => (assets as { clipSeconds?: Record<string, number> }).clipSeconds?.[name] ?? 0;
/** Where the clip's first frame sits in the song. */
export const songAt = (name: ClipName): number => assets.clips[name].songAt;

/**
 * A music-video clip, full frame. `start` is how far into the clip it begins (seconds). Past its last frame it holds
 * there. The picture only: the song is laid in by the audio plan in Film.tsx, at `songAt(name) + start`.
 */
export const ClipShot: React.FC<{ name: ClipName; start?: number; push?: number; total: number }> = ({ name, start = 0, push = 0.04, total }) => {
  const f = useCurrentFrame();
  const startFrame = Math.round(start * FPS);
  const last = Math.max(0, Math.round(clipSeconds(name) * FPS) - startFrame - 2);
  const scale = 1 + push * Math.min(1, f / Math.max(1, total));
  return (
    <AbsoluteFill style={{ background: '#000', overflow: 'hidden' }}>
      <Freeze frame={Math.min(f, last)}>
        <OffthreadVideo src={staticFile(`clips/${name}.mp4`)} startFrom={startFrame} muted style={{ width: W, height: H, objectFit: 'cover', transform: `scale(${scale})` }} />
      </Freeze>
    </AbsoluteFill>
  );
};

/**
 * The freeze and pull-back: a full-frame clip held on one frame, then the camera pulls back until the frame is
 * the video preview inside bro's storyboard (`box`, in the capture's pixels) on the desktop picture `picture`.
 * The clip is laid over the box in the picture, so the two stay in register all the way out.
 */
export const PullBack: React.FC<{ clip: ClipName; freezeAt: number; picture: string; box: Box; at: number; dur: number; end?: Cam }> = ({ clip, freezeAt, picture, box, at, dur, end }) => {
  const f = useCurrentFrame();
  const from = exact(box);
  const to: Cam = end ?? { cx: PIC_W / 2, cy: PIC_H / 2, z: 1 };
  const p = smooth(f, at, dur);
  const cam: Cam = { cx: mix(from.cx, to.cx, p), cy: mix(from.cy, to.cy, p), z: Math.exp(mix(Math.log(from.z), Math.log(to.z), p)) };
  const [x, y, w, h] = box;
  const startFrame = Math.round(freezeAt * FPS);
  return (
    <div style={worldStyle(cam)}>
      <Img src={staticFile(`desk/${picture}.png`)} style={{ position: 'absolute', left: 0, top: 0, width: PIC_W, height: PIC_H }} />
      <div style={{ position: 'absolute', left: x + OFF, top: y + OFF, width: w, height: h, overflow: 'hidden', borderRadius: 3 }}>
        <Freeze frame={0}>
          <OffthreadVideo src={staticFile(`clips/${clip}.mp4`)} startFrom={startFrame} muted style={{ width: w, height: h, objectFit: 'cover' }} />
        </Freeze>
      </div>
    </div>
  );
};

/** Layers for a time-lapse of captures: each picture dissolves into the next, evenly, from frame `a` to frame `b`. */
export const timelapse = (names: string[], a: number, b: number): Layer[] => {
  const n = names.length;
  const step = (b - a) / Math.max(1, n - 1);
  return names.map((src, i) => ({ src, at: i === 0 ? 0 : Math.round(a + (i - 1) * step), fade: Math.max(3, Math.round(step * 1.15)) }));
};

export { BASE, Sequence };
