import { loadFont as loadInter } from '@remotion/google-fonts/Inter';

/** Inter for everything read. */
export const FONT = loadInter('normal', { weights: ['400', '500', '600', '700', '800'], subsets: ['latin'] }).fontFamily;

/** The app's own colours: near-black panels, white text, hot pink as the one accent. */
export const C = {
  bg: '#0b0d11',
  panel: 'rgba(11,13,17,0.88)',
  panelSolid: '#12141a',
  line: 'rgba(255,255,255,0.16)',
  ink: '#ffffff',
  ink2: '#c9ccd4',
  muted: '#8f949f',
  pink: '#FF5FA8',
  pinkSoft: 'rgba(255,95,168,0.18)',
  pinkGlow: 'rgba(255,95,168,0.55)',
};

export const W = 1920;
export const H = 1080;
export const FPS = 30;
