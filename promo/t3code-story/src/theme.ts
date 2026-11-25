import { loadFont as loadInter } from '@remotion/google-fonts/Inter';
import { loadFont as loadMono } from '@remotion/google-fonts/JetBrainsMono';

/** Inter for everything read, JetBrains Mono (the mono bro ships) for commands, logs and small labels. */
export const FONT = loadInter('normal', { weights: ['400', '500', '600', '700'], subsets: ['latin'] }).fontFamily;
export const MONO = loadMono('normal', { weights: ['400', '500'], subsets: ['latin'] }).fontFamily;

/** bro's dark window, sampled from the Computers view, plus the orange of its icon. */
export const C = {
  bg: '#0b0d11',
  panel: '#15171d',
  panel2: '#1c1f27',
  well: '#0f1116',
  line: 'rgba(255,255,255,0.09)',
  lineStrong: 'rgba(255,255,255,0.18)',
  ink: '#f4f5f7',
  ink2: '#c8ccd4',
  muted: '#8f949f',
  faint: '#5d626d',
  accent: '#f6a04d',
  accentSoft: 'rgba(246,160,77,0.14)',
  accentLine: 'rgba(246,160,77,0.5)',
  blue: '#7fb2ff',
  blueSoft: 'rgba(127,178,255,0.13)',
  ok: '#8fcf9c',
  okSoft: 'rgba(143,207,156,0.13)',
  warn: '#e8c268',
  warnSoft: 'rgba(232,194,104,0.13)',
  danger: '#f17e7e',
  dangerSoft: 'rgba(241,126,126,0.14)',
};

export const W = 1920;
export const H = 1080;
/** Left and right edge of everything. */
export const MX = 120;
