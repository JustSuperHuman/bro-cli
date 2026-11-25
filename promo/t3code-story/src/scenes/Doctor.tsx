import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Card, Header, Icon, Label, Mark, Rise, SceneFC, Sfx, body, mono, prog } from '../ui';

/** What `Diagnosis::render` prints for a computer whose Bro is closed (bro-core/src/justconnect/health.rs). */
const CHECKS: { kind: 'ok' | 'fail'; name: string; said: string }[] = [
  { kind: 'ok', name: 'This computer', said: 'signed in and online' },
  { kind: 'ok', name: 'Just Superhuman', said: 'answering' },
  { kind: 'ok', name: 'Build box', said: 'on your account, access on' },
  { kind: 'fail', name: 'Bro there', said: 'closed · last open 2 hr ago' },
];
const STEPS = [
  'Open Bro on Build box. Sessions run inside it, so it has to be running to start or follow one.',
  "Can't get to it? Open its desktop from here: its screen comes up in your browser, and you can start Bro there.",
];
const COMMAND = 'bro computers doctor <id>';

const TermMark: React.FC<{ kind: 'ok' | 'fail' }> = ({ kind }) => (
  <span style={{ display: 'inline-block', width: 27, verticalAlign: '-4px' }}>
    <Icon name={kind === 'ok' ? 'check' : 'x'} size={22} color={kind === 'ok' ? C.ok : C.danger} stroke={2.6} />
  </span>
);

export const Doctor: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b10, b11 } = scene.beats;
  const checks = cue(b10, 'It checks');
  const stops = cue(b10, 'stops at the first');
  const fixes = cue(b10, 'with the steps');
  const rowAt = (i: number) => (i < 3 ? checks + i * Math.max(6, Math.round((stops - checks) / 3.4)) : stops);

  const typing = b11.from + 6;
  const typed = Math.max(0, Math.min(COMMAND.length, Math.floor((f - typing) / 1.3)));
  const printed = typing + Math.ceil(COMMAND.length * 1.3) + 8;
  const line = (i: number) => prog(f, printed + i * 3, 6);

  return (
    <>
      <Header eyebrow="Remote computers" titles={[{ at: 4, text: 'When it fails, bro names the link' }]} />

      <Rise at={b10.from - 4} style={{ position: 'absolute', left: MX, top: 246, width: 900 }}>
        <Card pad="40px 44px" style={{ height: 650 }}>
          <Label>In the Computers view</Label>
          <div style={{ ...body(44, C.ink, 600), letterSpacing: '-0.02em', marginTop: 14 }}>Bro is closed on Build box.</div>
          <div style={{ marginTop: 28, display: 'flex', flexDirection: 'column', gap: 18 }}>
            {CHECKS.map((c, i) => (
              <Rise key={c.name} at={rowAt(i)} by={12} dur={10} style={{ display: 'flex', alignItems: 'center', gap: 18 }}>
                <Mark kind={c.kind} size={42} />
                <div style={{ ...body(31, C.ink, 500), width: 268 }}>{c.name}</div>
                <div style={body(30, c.kind === 'fail' ? C.danger : C.muted)}>{c.said}</div>
              </Rise>
            ))}
          </div>
          <Rise at={fixes} style={{ marginTop: 38 }}>
            <Label>What to do</Label>
            <div style={{ marginTop: 14, display: 'flex', flexDirection: 'column', gap: 12 }}>
              {STEPS.map((step, i) => (
                <div key={i} style={{ display: 'flex', gap: 16 }}>
                  <span style={{ ...body(27, C.faint, 600), width: 24 }}>{i + 1}.</span>
                  <span style={body(27, C.ink2)}>{step}</span>
                </div>
              ))}
            </div>
          </Rise>
        </Card>
      </Rise>

      <Rise at={b11.from - 4} style={{ position: 'absolute', left: MX + 940, top: 246, width: 740 }}>
        <Card pad={0} style={{ height: 650, background: '#090a0d', overflow: 'hidden' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 12, height: 58, padding: '0 24px', borderBottom: `1.5px solid ${C.line}`, ...body(22, C.muted, 500) }}>
            <Icon name="terminal" size={24} color={C.muted} /> From a terminal
          </div>
          <div style={{ padding: '28px 28px', ...mono(22.5, C.ink2), lineHeight: 1.7, whiteSpace: 'pre' }}>
            <div>
              <span style={{ color: C.accent }}>$ </span>
              <span style={{ color: C.ink }}>{COMMAND.slice(0, typed)}</span>
              <span style={{ display: 'inline-block', width: 12, height: 26, verticalAlign: '-5px', background: C.ink2, opacity: f < printed && Math.floor(f / 8) % 2 === 0 ? 1 : 0 }} />
            </div>
            <div style={{ opacity: line(0), color: C.ink, marginTop: 14 }}>Bro is closed on Build box.</div>
            <div style={{ marginTop: 14 }}>
              {CHECKS.map((c, i) => (
                <div key={c.name} style={{ opacity: line(1 + i) }}>
                  {'  '}<TermMark kind={c.kind} />{c.name.padEnd(17)}<span style={{ color: c.kind === 'fail' ? C.danger : C.muted }}>{c.said}</span>
                </div>
              ))}
            </div>
            <div style={{ opacity: line(5), marginTop: 14, color: C.ink }}>What to do</div>
            <div style={{ opacity: line(6), whiteSpace: 'pre-wrap', paddingLeft: 26, color: C.muted }}>1. Open Bro on Build box. Sessions run inside it, so it has to be running to start or follow one.</div>
            <div style={{ opacity: line(7), marginTop: 14, color: C.ink }}>From a terminal here</div>
            <div style={{ opacity: line(8) }}>{'  '}bro computers desktop {'<id>'}</div>
          </div>
        </Card>
      </Rise>

      {CHECKS.map((c, i) => <Sfx key={i} at={rowAt(i)} name={c.kind === 'fail' ? 'impact-soft' : 'tick-bright'} volume={c.kind === 'fail' ? 0.08 : 0.07} />)}
    </>
  );
};
