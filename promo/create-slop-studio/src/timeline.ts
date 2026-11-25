import timing from './data/timing.json';

export const FPS = timing.fps;

export type Beat = {
  id: string;
  file: string | null;
  say: string;
  show: string;
  typed: string | null;
  /** Frame the line starts at, from the start of its scene. */
  from: number;
  frames: number;
  slot: number;
  /** Where speech begins and ends inside the audio, in frames from `from`. */
  begin: number;
  end: number;
};

export type Scene = {
  id: string;
  /** Frame the scene starts at in the whole film. */
  from: number;
  frames: number;
  beats: Beat[];
};

export const scenes: Scene[] = timing.scenes as Scene[];
export const TOTAL: number = timing.total;

export const sceneOf = (id: string): Scene => {
  const s = scenes.find((x) => x.id === id);
  if (!s) throw new Error(`no scene ${id}`);
  return s;
};

export const beatOf = (scene: Scene, id: string): Beat => {
  const b = scene.beats.find((x) => x.id === id);
  if (!b) throw new Error(`no line ${id} in ${scene.id}`);
  return b;
};

/** The frame (from the scene's start) at which `phrase` is spoken in a line, judged by how far into the text it is. */
export const cue = (scene: Scene, id: string, phrase: string): number => {
  const beat = beatOf(scene, id);
  const i = beat.say.toLowerCase().indexOf(phrase.toLowerCase());
  if (i < 0) throw new Error(`"${phrase}" isn't in ${beat.id}: ${beat.say}`);
  return Math.round(beat.from + beat.begin + (i / beat.say.length) * (beat.end - beat.begin));
};

/** The frame a line starts (+ `plus`), from the scene's start. */
export const at = (scene: Scene, id: string, plus = 0): number => beatOf(scene, id).from + plus;
