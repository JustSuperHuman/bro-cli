import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C, MX } from '../theme';
import { Header, Icon, Label, SceneFC, Sfx, body, mix, prog } from '../ui';

const ROWS = [
  {
    beat: 'b22', icon: 'server', tone: C.blue, soft: C.blueSoft, label: 'When', when: 'The account server change is deployed',
    expect: 'Remote work starts the moment you ask',
    detail: 'bro notices by itself and stops using the clock. Up to 64 requests are held open at a time.',
  },
  {
    beat: 'b23', icon: 'monitor', tone: C.ok, soft: C.okSoft, label: 'When', when: 'A second computer joins your account',
    expect: 'The first real run between two machines',
    detail: 'So far the worker has only run against a stand-in server in tests.',
  },
  {
    beat: 'b24', icon: 'clock', tone: C.accent, soft: C.accentSoft, label: 'When', when: 'The new log has a day of use',
    expect: 'It shows where the long stalls sit',
    detail: 'Every slow frame now records acquire, draw and present.',
  },
  {
    beat: 'b25', icon: 'wrench', tone: C.ink2, soft: 'rgba(255,255,255,0.07)', label: 'Not built yet', when: 'Two pieces of the remote path',
    expect: 'Start and answer remote sessions from the Computers view',
    detail: 'And a relay in the account server, for live remote terminals.',
  },
];

export const Next: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b21 } = scene.beats;
  const starts = ROWS.map((r) => scene.beats[r.beat].from - 4);

  return (
    <>
      <Header eyebrow="Next" titles={[{ at: 4, text: 'What to expect from here' }]} />
      <div style={{ position: 'absolute', left: MX, top: 244, width: 1680, display: 'flex', flexDirection: 'column', gap: 18 }}>
        {ROWS.map((row, i) => {
          // the three conditions are laid out as they are announced; each one's outcome arrives with its own line
          const shown = prog(f, i < 3 ? b21.from + 10 + i * 9 : starts[i], 16);
          const told = prog(f, starts[i], 16);
          const later = starts[i + 1] === undefined ? 0 : prog(f, starts[i + 1], 14);
          const active = told * (1 - later);
          return (
            <div
              key={row.beat}
              style={{
                display: 'flex', alignItems: 'center', gap: 32, height: 154, padding: '0 38px', boxSizing: 'border-box', borderRadius: 22,
                background: active > 0.5 ? C.panel2 : C.panel, border: `1.5px solid rgba(255,255,255,${mix(0.09, 0.2, active)})`,
                opacity: shown * mix(0.6, 1, active), transform: `translateY(${(1 - shown) * 22}px)`,
              }}
            >
              <span style={{ width: 76, height: 76, borderRadius: 20, background: row.soft, display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
                <Icon name={row.icon} size={40} color={row.tone} />
              </span>
              <div style={{ width: 470, flex: 'none' }}>
                <Label style={{ fontSize: 18 }}>{row.label}</Label>
                <div style={{ ...body(28, C.ink, 500), marginTop: 6, lineHeight: 1.25 }}>{row.when}</div>
              </div>
              <div style={{ width: 1.5, alignSelf: 'stretch', margin: '30px 0', background: C.line }} />
              <div style={{ flex: 1, opacity: told, transform: `translateX(${(1 - told) * 18}px)` }}>
                <div style={{ ...body(35, C.ink, 600), letterSpacing: '-0.02em', lineHeight: 1.2 }}>{row.expect}</div>
                <div style={{ ...body(25, C.muted), marginTop: 6 }}>{row.detail}</div>
              </div>
            </div>
          );
        })}
      </div>
      {starts.map((at, i) => <Sfx key={i} at={at} name="pop" volume={0.6} />)}
    </>
  );
};
