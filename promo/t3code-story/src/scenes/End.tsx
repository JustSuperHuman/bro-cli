import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C } from '../theme';
import { Icon, Pill, Rise, SceneFC, Sfx, body, mono, prog } from '../ui';

export const End: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b26 } = scene.beats;
  const p = prog(f, 4, 24);
  return (
    <>
      <div style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', paddingBottom: 90 }}>
        <div style={{ opacity: p, transform: `scale(${0.86 + 0.14 * p})` }}>
          <Icon name="bro" size={210} />
        </div>
        <Rise at={10} by={26}>
          <div style={{ fontSize: 132, fontWeight: 600, letterSpacing: '-0.045em', lineHeight: 1.1, marginTop: 10 }}>
            bro <span style={{ color: C.accent, fontVariantNumeric: 'tabular-nums' }}>2.1.49</span>
          </div>
        </Rise>
        <Rise at={b26.from + 20} style={{ marginTop: 22 }}>
          <Pill tone="ok" size={30} dot>Installed on this computer</Pill>
        </Rise>
        <Rise at={b26.from + 44} style={{ marginTop: 44, textAlign: 'center' }}>
          <div style={body(27, C.muted)}>The full write-up, with the comparison tables</div>
          <div style={{ ...mono(25, C.ink2), marginTop: 8 }}>reports/t3code-deep-dive/index.html</div>
        </Rise>
      </div>
      <Sfx at={8} name="chime-success" volume={0.12} />
    </>
  );
};
