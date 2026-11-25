import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C, MONO, MX } from '../theme';
import { cue } from '../timeline';
import { Card, Icon, Label, Rise, SceneFC, Sfx, body, mix, prog } from '../ui';

const TRACKS = [
  { icon: 'monitor', title: 'Remote computers', line: 'How it reaches another machine and keeps the link alive' },
  { icon: 'clock', title: 'Speed', line: 'What keeps it quick when sessions pile up' },
  { icon: 'wrench', title: 'Small things', line: 'Safeguards and details that are easy to miss' },
];

/** The ways T3 Code can reach a computer. bro keeps the last one. */
const ROUTES = ['SSH', 'Tailscale', 'LAN', 'VPN', 'Public address', 'Loopback', 'Account relay'];

export const Intro: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b01, b02 } = scene.beats;
  const tracks = cue(b01, 'looking for');
  const swap = b02.from - 10;
  const routes = b02.from + 2;
  const pick = cue(b02, 'it goes through');
  const picked = prog(f, pick, 16);

  return (
    <>
      {/* b01: what was read, and what for */}
      <Rise at={0} out={swap} by={0} style={{ position: 'absolute', left: MX, top: 150, width: 1680 }}>
        <Rise at={4}>
          <Label style={{ fontSize: 22 }}>T3 Code, read end to end</Label>
        </Rise>
        <Rise at={8} by={30}>
          <div style={{ fontSize: 118, fontWeight: 600, letterSpacing: '-0.04em', lineHeight: 1.04, marginTop: 22 }}>
            What bro took
            <br />
            from T3 Code
          </div>
        </Rise>
        <Rise at={20}>
          <div style={{ ...body(30, C.muted), marginTop: 28 }}>
            <span style={{ fontFamily: MONO, fontSize: 27, color: C.ink2 }}>pingdotgg/t3code</span> at commit <span style={{ fontFamily: MONO, fontSize: 27, color: C.ink2 }}>047d492</span>, read on 10 November 2026
          </div>
        </Rise>
        <div style={{ display: 'flex', gap: 28, marginTop: 84 }}>
          {TRACKS.map((t, i) => (
            <Rise key={t.title} at={tracks + i * 7} style={{ flex: 1 }}>
              <Card pad="30px 32px" style={{ display: 'flex', gap: 24, alignItems: 'flex-start', height: 170 }}>
                <span style={{ width: 60, height: 60, borderRadius: 16, background: C.accentSoft, display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
                  <Icon name={t.icon} size={32} color={C.accent} />
                </span>
                <div>
                  <div style={{ ...body(33, C.ink, 600), letterSpacing: '-0.015em' }}>{t.title}</div>
                  <div style={{ ...body(24, C.muted), marginTop: 6 }}>{t.line}</div>
                </div>
              </Card>
            </Rise>
          ))}
        </div>
      </Rise>

      {/* b02: the rule */}
      <div style={{ position: 'absolute', left: MX, top: 150, width: 1680 }}>
        <Rise at={swap + 8}>
          <Label style={{ fontSize: 22 }}>The rule</Label>
        </Rise>
        <Rise at={swap + 10} by={26}>
          <div style={{ fontSize: 84, fontWeight: 600, letterSpacing: '-0.035em', lineHeight: 1.08, marginTop: 22 }}>
            Another computer is reached
            <br />
            through your account
          </div>
        </Rise>
        <Rise at={routes + 8}>
          <Label style={{ marginTop: 76 }}>T3 Code's ways in</Label>
        </Rise>
        <div style={{ display: 'flex', gap: 16, marginTop: 22, alignItems: 'center' }}>
          {ROUTES.map((name, i) => {
            const kept = i === ROUTES.length - 1;
            const shown = prog(f, routes + 12 + i * 3, 12);
            return (
              <div
                key={name}
                style={{
                  position: 'relative', height: 70, padding: '0 30px', borderRadius: 999, display: 'flex', alignItems: 'center', boxSizing: 'border-box',
                  border: `1.5px solid ${kept ? `rgba(127,178,255,${mix(0.12, 0.6, picked)})` : C.line}`,
                  background: kept ? `rgba(127,178,255,${mix(0.02, 0.14, picked)})` : C.panel,
                  ...body(30, kept ? C.ink : C.ink2, 500),
                  opacity: shown * (kept ? 1 : mix(1, 0.34, picked)),
                  transform: `translateY(${(1 - shown) * 16}px)`,
                }}
              >
                {name}
                {kept ? null : <div style={{ position: 'absolute', left: 22, right: 22, top: '50%', height: 2, background: C.muted, transformOrigin: 'left center', transform: `scaleX(${picked})` }} />}
              </div>
            );
          })}
        </div>
        <Rise at={pick + 14} style={{ marginTop: 44 }}>
          <Card tone="blue" pad="30px 36px" style={{ display: 'flex', alignItems: 'center', gap: 28 }}>
            <Icon name="knot" size={64} color={C.blue} />
            <div>
              <div style={{ ...body(40, C.ink, 600), letterSpacing: '-0.02em' }}>Your Just Superhuman account</div>
              <div style={{ ...body(27, C.ink2), marginTop: 4 }}>bro keeps this one. SSH, Tailscale and typed addresses stay out.</div>
            </div>
          </Card>
        </Rise>
      </div>

      <Sfx at={tracks} name="pop" volume={0.6} />
      <Sfx at={pick} name="tick-bright" volume={0.08} />
    </>
  );
};
