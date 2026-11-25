import timing from './data/timing.json';

export const FPS = 30;
/** Quiet before a scene's first line, between two lines, and after its last one (seconds). */
const LEAD = 0.55;
const GAP = 0.45;
const TAIL = 0.95;

export type Beat = {
  id: string;
  scene: string;
  file: string | null;
  /** What is spoken, and what the caption shows (numbers as figures). */
  say: string;
  show: string;
  /** Frame the line's audio starts at, from the start of its scene. */
  from: number;
  /** Frames the audio lasts. */
  frames: number;
  /** Frames until the next line starts, or the scene ends. */
  slot: number;
  /** Where speech begins and ends inside the audio, in frames from `from`. */
  begin: number;
  end: number;
};

export type Scene = {
  id: string;
  /** Frame the scene starts at in the whole story. */
  from: number;
  frames: number;
  beats: Record<string, Beat>;
  list: Beat[];
};

const f = (seconds: number) => Math.round(seconds * FPS);

export const scenes: Scene[] = [];
for (const b of timing.beats) {
  let scene = scenes[scenes.length - 1];
  if (!scene || scene.id !== b.scene) {
    scene = { id: b.scene, from: 0, frames: f(LEAD), beats: {}, list: [] };
    scenes.push(scene);
  }
  const frames = f(b.seconds);
  const slot = Math.max(frames + f(GAP), f(b.min ?? 0));
  const beat: Beat = { id: b.id, scene: b.scene, file: b.file, say: b.say, show: b.show, from: scene.frames, frames, slot, begin: f(b.begin), end: f(b.end) };
  scene.frames += slot;
  scene.beats[b.id] = beat;
  scene.list.push(beat);
}
let at = 0;
for (const scene of scenes) {
  scene.frames += f(TAIL - GAP);
  scene.from = at;
  at += scene.frames;
}

export const TOTAL = at;

/** The frame (from the scene's start) at which `phrase` is spoken in a line, judged by how far into the text it is. */
export const cue = (beat: Beat, phrase: string): number => {
  const i = beat.say.toLowerCase().indexOf(phrase.toLowerCase());
  if (i < 0) throw new Error(`"${phrase}" isn't in ${beat.id}: ${beat.say}`);
  return Math.round(beat.from + beat.begin + (i / beat.say.length) * (beat.end - beat.begin));
};

/** Which part of the story a scene belongs to, for the marker in the corner. */
export const CHAPTERS = ['Remote computers', 'Speed', 'Small things', 'Next'] as const;
export const chapterOf = (scene: string): number => ({ path: 0, pool: 0, doctor: 0, view: 0, log: 1, stall: 1, small: 2, next: 3, end: 3 } as Record<string, number>)[scene] ?? -1;
