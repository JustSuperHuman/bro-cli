import { Easing, interpolate } from 'remotion';

const clamp = { extrapolateLeft: 'clamp', extrapolateRight: 'clamp' } as const;

export const ease = Easing.bezier(0.2, 0.7, 0.2, 1);
/** Slow in, slow out: for camera moves. */
export const inOut = Easing.bezier(0.45, 0, 0.2, 1);

/** 0 to 1 over `dur` frames from `start`, eased out. */
export const prog = (frame: number, start: number, dur = 16) => interpolate(frame, [start, start + dur], [0, 1], { ...clamp, easing: ease });
/** The same without easing. */
export const lin = (frame: number, start: number, dur: number) => interpolate(frame, [start, start + dur], [0, 1], clamp);
/** 0 to 1 with slow start and end, for moves of the camera and the pointer. */
export const smooth = (frame: number, start: number, dur: number) => interpolate(frame, [start, start + dur], [0, 1], { ...clamp, easing: inOut });
export const mix = (a: number, b: number, t: number) => a + (b - a) * t;
