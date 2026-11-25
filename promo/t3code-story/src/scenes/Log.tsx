import React from 'react';
import { useCurrentFrame } from 'remotion';
import perf from '../data/perf.json';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Header, Icon, Label, Rise, SceneFC, Sfx, body, mono, prog } from '../ui';

const B = perf.before;
const WIDTH = 1680;
const fill: Record<string, string> = { gpu: C.accent, render: 'rgba(200,204,212,0.62)', unattributed: 'rgba(143,148,159,0.42)', housekeeping: 'rgba(143,148,159,0.25)', events: 'rgba(143,148,159,0.25)' };

/** Where a duration sits on the ruler: a log scale from 50 ms to 10 s. */
const at = (ms: number) => (Math.log(ms / 50) / Math.log(10000 / 50)) * WIDTH;
const MARKS = [50, 100, 200, 500, 1000, 2000, 5000, 10000];
const said = (ms: number) => (ms >= 1000 ? `${ms / 1000} s` : `${ms} ms`);

export const Log: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b14, b15 } = scene.beats;
  const counts = cue(b14, 'Over fourteen');
  const count = Math.round(B.total * prog(f, counts + 6, cue(b14, 'slow passes') - counts - 6));
  const split = cue(b15, 'About three quarters');
  const most = cue(b15, 'Most lasted');
  const few = cue(b15, 'A few ran');
  const keys = cue(b15, 'your keys');
  const grow = prog(f, split, 30);
  const gpu = B.parts[0];
  let x = 0;

  return (
    <>
      <Header eyebrow="Speed" titles={[{ at: 4, text: "The biggest finding was in bro's own log" }]} />

      <div style={{ position: 'absolute', left: MX, top: 236, width: WIDTH, display: 'flex', alignItems: 'flex-end', gap: 44 }}>
        <Rise at={counts - 6} by={30}>
          <div style={{ fontSize: 210, fontWeight: 600, letterSpacing: '-0.05em', lineHeight: 1, fontVariantNumeric: 'tabular-nums', minWidth: 560 }}>{count.toLocaleString('en-US')}</div>
        </Rise>
        <Rise at={counts + 4} style={{ paddingBottom: 22 }}>
          <div style={{ ...body(46, C.ink, 600), letterSpacing: '-0.02em' }}>slow passes in {B.hours}{' '}hours</div>
          <div style={{ ...body(27, C.muted), marginTop: 6 }}>
            {B.from} to {B.to}. A pass is one turn of the app's main loop; slow means over 50 ms.
          </div>
        </Rise>
      </div>

      {/* what the slow passes were spent on */}
      <Rise at={split - 6} style={{ position: 'absolute', left: MX, top: 500, width: WIDTH }}>
        <div style={{ display: 'flex', height: 54, borderRadius: 14, overflow: 'hidden', background: 'rgba(255,255,255,0.05)' }}>
          {B.parts.map((p) => (
            <div key={p.key} style={{ width: (p.count / B.total) * WIDTH * grow, height: '100%', background: fill[p.key], borderRight: `2px solid ${C.bg}`, boxSizing: 'border-box' }} />
          ))}
        </div>
        <div style={{ position: 'relative', height: 80, marginTop: 16 }}>
          {B.parts.slice(0, 3).map((p, i) => {
            const left = x;
            x += (p.count / B.total) * WIDTH;
            return (
              <Rise key={p.key} at={split + 10 + i * 6} by={10} style={{ position: 'absolute', left, top: 0, whiteSpace: 'nowrap' }}>
                <div style={{ ...body(i === 0 ? 34 : 27, i === 0 ? C.accent : C.ink2, 600), fontVariantNumeric: 'tabular-nums' }}>{p.count.toLocaleString('en-US')}</div>
                <div style={body(i === 0 ? 26 : 22, C.muted)}>{i === 0 ? `${p.label}: ${Math.round((p.count / B.total) * 100)}% of them` : p.label}</div>
              </Rise>
            );
          })}
        </div>
      </Rise>

      {/* how long the graphics waits lasted */}
      <Rise at={most - 8} style={{ position: 'absolute', left: MX, top: 690, width: WIDTH }}>
        <Label>How long a wait on graphics lasted</Label>
        <div style={{ position: 'relative', height: 100, marginTop: 34 }}>
          <div style={{ position: 'absolute', left: 0, right: 0, top: 44, height: 2, background: C.lineStrong }} />
          {MARKS.map((ms) => (
            <div key={ms} style={{ position: 'absolute', left: at(ms) - 60, top: 38, width: 120, textAlign: 'center' }}>
              <div style={{ width: 2, height: 14, background: C.lineStrong, margin: '0 auto' }} />
              <div style={{ ...mono(20, C.faint), marginTop: 8 }}>{said(ms)}</div>
            </div>
          ))}
          {/* nine in ten end before 115 ms */}
          <div style={{ position: 'absolute', left: 0, top: 26, height: 36, width: at(gpu.p90) * prog(f, most, 18), borderRadius: 10, background: C.accent, opacity: 0.85 }} />
          <Rise at={most + 10} by={8} style={{ position: 'absolute', left: at(gpu.p90) + 18, top: -14, whiteSpace: 'nowrap' }}>
            <span style={body(26, C.ink, 600)}>Typical: {gpu.p50} ms.</span> <span style={body(26, C.muted)}>Nine in ten were over within {gpu.p90} ms.</span>
          </Rise>
          {/* a few ran for seconds */}
          <div style={{ position: 'absolute', left: at(5000), top: 26, height: 36, width: (at(gpu.max) - at(5000)) * prog(f, few, 18), borderRadius: 10, background: C.danger, opacity: 0.9 }} />
          <Rise at={few + 10} by={8} style={{ position: 'absolute', right: 0, top: -14, whiteSpace: 'nowrap', textAlign: 'right' }}>
            <span style={body(26, C.danger, 600)}>A few: 5 to 9 s.</span> <span style={body(26, C.muted)}>Longest {(gpu.max / 1000).toFixed(1)} s.</span>
          </Rise>
        </div>
      </Rise>

      <Rise at={keys - 4} style={{ position: 'absolute', left: MX, top: 884, width: WIDTH }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 16, ...body(28, C.ink2) }}>
          <Icon name="key" size={36} color={C.ink2} />
          Keys, the pointer and the phone's input wait on that same thread.
        </div>
      </Rise>

      <Sfx at={counts} name="whoosh-short" volume={0.07} />
      <Sfx at={few} name="impact-soft" volume={0.07} />
    </>
  );
};
