import React from 'react';
import { AbsoluteFill, Img, staticFile, useCurrentFrame } from 'remotion';
import { Muse } from './muse';
import { C, FONT } from './theme';
import { Scene } from './timeline';
import { prog } from './ui';

const Key: React.FC<{ at: number; children: string }> = ({ at, children }) => {
  const f = useCurrentFrame();
  const p = prog(f, at, 14);
  const press = prog(f, at + 14, 6) * (1 - prog(f, at + 20, 8));
  return (
    <div style={{ minWidth: 116, height: 96, padding: '0 30px', boxSizing: 'border-box', display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: 18, background: '#171a21', border: '2px solid rgba(255,255,255,0.34)', boxShadow: `0 ${8 - press * 5}px 0 rgba(255,255,255,0.12), 0 22px 40px rgba(0,0,0,0.5)`, color: C.ink, fontSize: 44, fontWeight: 600, letterSpacing: '-0.01em', opacity: p, transform: `translateY(${(1 - p) * 26 + press * 5}px)` }}>
      {children}
    </div>
  );
};

/** The end card: "bro." in type, Create Slop studio, the version, the keys. Over the desktop wallpaper, with Meta Muse. */
export const EndCard: React.FC<{ scene: Scene }> = () => {
  const f = useCurrentFrame();
  const wordmark = prog(f, 3, 18);
  const title = prog(f, 14, 18);
  const ver = prog(f, 26, 16);
  const keys = 40;
  return (
    <AbsoluteFill style={{ background: '#05060a', fontFamily: FONT }}>
      <Img src={staticFile('img/wall.jpg')} style={{ position: 'absolute', inset: 0, width: '100%', height: '100%' }} />
      <AbsoluteFill style={{ background: 'radial-gradient(900px 600px at 26% 80%, rgba(255,95,168,0.22), transparent 70%)' }} />
      <Muse at={0} corner="bl" width={760} poses={[[0, 'smirk'], [keys + 20, 'point']]} />
      <div style={{ position: 'absolute', left: 900, top: 150, width: 900 }}>
        <div style={{ fontSize: 150, fontWeight: 700, letterSpacing: '-0.05em', lineHeight: 1, color: C.ink, opacity: wordmark, transform: `translateY(${(1 - wordmark) * 24}px)` }}>
          bro<span style={{ color: C.pink }}>.</span>
        </div>
        <div style={{ marginTop: 22, fontSize: 92, fontWeight: 700, letterSpacing: '-0.035em', lineHeight: 1.04, color: C.ink, opacity: title, transform: `translateY(${(1 - title) * 28}px)` }}>
          Create Slop
          <br />
          studio
        </div>
        <div style={{ marginTop: 40, display: 'inline-block', padding: '12px 34px 14px', borderRadius: 999, background: '#EC3F8C', color: '#ffffff', fontSize: 50, fontWeight: 700, letterSpacing: '-0.02em', opacity: ver, transform: `translateY(${(1 - ver) * 20}px)` }}>2.1.56</div>
        <div style={{ marginTop: 52, display: 'flex', gap: 22, alignItems: 'center' }}>
          <Key at={keys}>alt</Key>
          <Key at={keys + 7}>shift</Key>
          <Key at={keys + 14}>S</Key>
        </div>
      </div>
    </AbsoluteFill>
  );
};
