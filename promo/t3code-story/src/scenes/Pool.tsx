import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Card, Header, Icon, Label, Mark, Pill, Rise, SceneFC, Sfx, body, lin, mono, prog } from '../ui';

/** One request the old way: its own handshake, then the request. */
const Separate: React.FC<{ at: number }> = ({ at }) => {
  const f = useCurrentFrame();
  const shake = lin(f, at, 16);
  const request = lin(f, at + 16, 9);
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 12, height: 50, opacity: prog(f, at - 4, 8) }}>
      <Icon name="lock" size={30} color={C.warn} />
      <div style={{ width: 300, height: 18, borderRadius: 9, background: 'rgba(232,194,104,0.14)', overflow: 'hidden' }}>
        <div style={{ width: `${shake * 100}%`, height: '100%', borderRadius: 9, background: C.warn, opacity: 0.8 }} />
      </div>
      <div style={{ width: 170, height: 18, borderRadius: 9, background: 'rgba(255,255,255,0.07)', overflow: 'hidden' }}>
        <div style={{ width: `${request * 100}%`, height: '100%', borderRadius: 9, background: C.ink2 }} />
      </div>
    </div>
  );
};

/** A job on the other computer: a name and a bar that fills over `frames`. */
const Job: React.FC<{ name: string; at: number; frames: number; slow?: boolean }> = ({ name, at, frames, slow }) => {
  const f = useCurrentFrame();
  const p = lin(f, at, frames);
  const done = p >= 1;
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 18, height: 54, opacity: prog(f, at - 6, 8) }}>
      <div style={{ ...mono(24, slow ? C.accent : C.ink2), width: 210 }}>{name}</div>
      <div style={{ flex: 1, height: 16, borderRadius: 8, background: 'rgba(255,255,255,0.07)', overflow: 'hidden' }}>
        <div style={{ width: `${p * 100}%`, height: '100%', borderRadius: 8, background: slow ? C.accent : C.ok, opacity: done ? 0.55 : 1 }} />
      </div>
      <div style={{ width: 34, opacity: done ? 1 : 0 }}><Mark kind="ok" size={32} /></div>
    </div>
  );
};

export const Pool: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b08, b09 } = scene.beats;
  const old = cue(b08, 'Each request used to');
  const shared = cue(b08, 'They now share');
  const also = cue(b08, 'along with');
  const four = cue(b09, 'up to four jobs');
  const order = cue(b09, 'Keystrokes');

  return (
    <>
      <Header
        eyebrow="Remote computers"
        titles={[
          { at: 4, text: 'One connection, shared' },
          { at: b09.from - 4, text: 'Four jobs at a time' },
        ]}
      />

      <Rise at={10} dim={{ at: b09.from, to: 0.5 }} style={{ position: 'absolute', left: MX, top: 246, width: 810 }}>
        <Card pad="36px 40px" style={{ height: 650 }}>
          <div style={{ ...body(34, C.ink, 600), letterSpacing: '-0.015em' }}>Requests to your account</div>

          <Rise at={old - 4} style={{ marginTop: 30 }}>
            <Label>Before</Label>
            <div style={{ display: 'flex', gap: 12, marginTop: 14, ...body(21, C.faint) }}>
              <span style={{ width: 30 }} />
              <span style={{ width: 300, whiteSpace: 'nowrap' }}>new encrypted connection</span>
              <span>request</span>
            </div>
            <div style={{ marginTop: 6 }}>
              {[0, 1, 2].map((i) => <Separate key={i} at={old + 8 + i * 22} />)}
            </div>
          </Rise>

          <Rise at={shared - 2} style={{ marginTop: 34 }}>
            <Label>Now</Label>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 18, height: 44 }}>
              <Icon name="lock" size={30} color={C.ok} />
              <div style={{ width: 86, height: 18, borderRadius: 9, background: C.ok, opacity: 0.75, transformOrigin: 'left center', transform: `scaleX(${lin(f, shared + 2, 10)})` }} />
              <div style={{ position: 'relative', flex: 1, height: 30, borderRadius: 15, border: `1.5px solid rgba(143,207,156,0.45)`, background: C.okSoft, overflow: 'hidden' }}>
                {[0, 1, 2, 3].map((i) => {
                  const x = ((Math.max(0, f - shared - 12) * 6 + i * 150) % 600) - 30;
                  return <div key={i} style={{ position: 'absolute', left: x, top: 6, width: 54, height: 15, borderRadius: 8, background: C.ink2, opacity: f > shared + 12 ? 1 : 0 }} />;
                })}
              </div>
            </div>
            <div style={{ ...body(24, C.muted), marginTop: 14 }}>One pool for every request. An idle connection is kept for 50 s.</div>
          </Rise>

          <Rise at={also} style={{ marginTop: 34 }}>
            <Label>Also on the pool</Label>
            <div style={{ display: 'flex', gap: 12, marginTop: 14 }}>
              {['Sign-in', 'Usage checks', 'Downloads'].map((name) => <Pill key={name} size={25}>{name}</Pill>)}
            </div>
          </Rise>
        </Card>
      </Rise>

      <Rise at={b09.from - 2} style={{ position: 'absolute', left: MX + 870, top: 246, width: 810 }}>
        <Card tone="raised" pad="36px 40px" style={{ height: 650 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <div style={{ ...body(34, C.ink, 600), letterSpacing: '-0.015em' }}>On the other computer</div>
            <Pill tone="accent" size={22}>up to 4 at once</Pill>
          </div>
          <div style={{ marginTop: 22 }}>
            <Job name="open_session" at={four} frames={190} slow />
            <Job name="list_sessions" at={four} frames={30} />
            <Job name="read_chat" at={four} frames={46} />
            <Job name="read_session" at={four} frames={38} />
            <Job name="list_projects" at={four + 58} frames={28} />
            <Job name="read_session" at={four + 70} frames={40} />
          </div>
          <div style={{ height: 1.5, background: C.line, margin: '18px 0 20px' }} />
          <Rise at={order - 4}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              <Label>Input for one session</Label>
              <Pill size={22}>in the order it was asked</Pill>
            </div>
            <div style={{ display: 'flex', gap: 14, marginTop: 18 }}>
              {['send_input', 'send_keys', 'answer_prompt'].map((name, i) => {
                const p = lin(f, order + 8 + i * 24, 22);
                return (
                  <div key={name} style={{ flex: 1, position: 'relative', height: 62, borderRadius: 14, border: `1.5px solid ${C.line}`, background: C.well, overflow: 'hidden', display: 'flex', alignItems: 'center', gap: 12, padding: '0 18px', boxSizing: 'border-box' }}>
                    <div style={{ position: 'absolute', left: 0, top: 0, bottom: 0, width: `${p * 100}%`, background: C.okSoft }} />
                    <span style={{ ...mono(20, C.faint), position: 'relative' }}>{i + 1}</span>
                    <span style={{ ...mono(22, p > 0 ? C.ink : C.muted), position: 'relative' }}>{name}</span>
                  </div>
                );
              })}
            </div>
          </Rise>
        </Card>
      </Rise>

      <Sfx at={shared} name="pop" volume={0.6} />
      <Sfx at={four} name="tick-bright" volume={0.07} />
    </>
  );
};
