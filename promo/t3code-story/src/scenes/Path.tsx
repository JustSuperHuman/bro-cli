import React from 'react';
import { useCurrentFrame } from 'remotion';
import { Chain, Packet, R, nodeX } from '../chain';
import { C, MX } from '../theme';
import { FPS, cue } from '../timeline';
import { Card, Header, Icon, Label, Mark, Pill, Rise, SceneFC, Sfx, body, mix, prog } from '../ui';

const Y = 418;
/** What each link is, under its name. The other computer's line changes with what it is doing. */
const WHAT = ['signed in, with a way out', "your account's server", 'on your account, switched on', 'runs the sessions'];

const Stat: React.FC<{ label: string; value: string; note: string; color?: string }> = ({ label, value, note, color = C.ink }) => (
  <div>
    <Label>{label}</Label>
    <div style={{ fontSize: 104, fontWeight: 600, letterSpacing: '-0.04em', lineHeight: 1.1, color, marginTop: 6, fontVariantNumeric: 'tabular-nums' }}>{value}</div>
    <div style={{ ...body(28, C.muted), marginTop: 2 }}>{note}</div>
  </div>
);

/** A dot that flashes `perSecond` times a second, so four clocks side by side can be compared by eye. */
const Tick: React.FC<{ perSecond: number; from: number }> = ({ perSecond, from }) => {
  const f = useCurrentFrame();
  const beats = Math.max(0, f - from) * (perSecond / FPS);
  const phase = beats - Math.floor(beats);
  const glow = f < from ? 0 : Math.exp(-phase * (perSecond > 2 ? 2.2 : 5));
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 14, height: 30 }}>
      <div style={{ width: 22, height: 22, borderRadius: 999, background: C.accent, opacity: 0.22 + 0.78 * glow, transform: `scale(${0.82 + 0.3 * glow})`, boxShadow: `0 0 ${22 * glow}px rgba(246,160,77,0.7)` }} />
      <div style={{ flex: 1, height: 2, background: C.line, borderRadius: 2 }} />
    </div>
  );
};

export const Path: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b03, b04, b05, b06, b07 } = scene.beats;

  // b03: the four links are named one after another
  const named = [cue(b03, 'this computer'), cue(b03, 'Just Superhuman'), cue(b03, 'the other computer'), cue(b03, 'and Bro running')];
  const toAccount = prog(f, b06.from - 4, 14) * (1 - prog(f, b07.from - 4, 14));
  const lit = named.map((at, i) => {
    const on = prog(f, at, 12);
    const aside = i === 1 ? 0 : toAccount * 0.62;
    return mix(0.12, 1, on) * (1 - aside);
  });
  const link = [0, 1, 2].map((i) => prog(f, named[i] + 4, Math.max(10, named[i + 1] - named[i] - 4)));

  // b04: the other computer asks every five seconds, and work waits at the account for the next ask
  const period = 5 * FPS;
  const asks = b04.from + 4;
  const ringOpacity = prog(f, asks - 6, 10) * (1 - prog(f, b05.from - 10, 10));
  const ringProgress = f < asks ? 0 : ((f - asks) % period) / period;
  const sent = asks + 22;
  const answered = asks + period;
  const before =
    f < sent ? -1 : f < answered ? prog(f, sent, 16) : f < answered + 16 ? 1 + prog(f, answered, 16) : 2 + prog(f, answered + 16, 16);
  const beforeOpacity = prog(f, sent - 2, 6) * (1 - prog(f, answered + 40, 10));
  const waited = Math.min(Math.max(0, f - (sent + 16)), answered - (sent + 16)) / FPS;
  const waitingOpacity = prog(f, sent + 14, 8) * (1 - prog(f, answered + 34, 10));

  // b05: one request stays open; work goes straight through
  const heldOpacity = prog(f, b05.from, 14) * (1 - prog(f, b06.from - 4, 12) * 0.55) * (1 - prog(f, b07.from - 4, 12));
  const arrives = cue(b05, 'When work comes in');
  const now = f < arrives ? -1 : prog(f, arrives, 14) + prog(f, arrives + 14, 14) + prog(f, arrives + 28, 14);
  const nowOpacity = prog(f, arrives - 2, 6) * (1 - prog(f, arrives + 56, 10));
  const reopened = arrives + 34;
  const heldFor = f < reopened && f >= arrives + 28 ? 0 : (f - (f >= reopened ? reopened : b05.from)) / FPS;
  const countdown = Math.max(0, 25 - heldFor);

  // b06 and b07 take over the space under the chain
  const statsOut = b06.from - 8;
  const statusOut = b07.from - 8;
  const steps = [cue(b07, 'four times a second') - 8, cue(b07, 'eases off'), cue(b07, 'eases off') + 9, cue(b07, 'once every five')];

  return (
    <>
      <Header
        eyebrow="Remote computers"
        titles={[
          { at: 4, text: 'A remote request crosses four links' },
          { at: b04.from - 4, text: 'Before: work waited for the next ask' },
          { at: b05.from - 4, text: 'Now: one request stays open' },
          { at: b06.from - 4, text: 'That half is waiting on a deploy' },
          { at: b07.from - 4, text: 'Until then, a clock that speeds up' },
        ]}
      />

      <Chain
        y={Y}
        lit={lit}
        link={link}
        sub={WHAT.map((what, i) => (i !== 2 ? (prog(f, named[i] + 4, 10) > 0.5 ? what : null) : ringOpacity > 0.5 ? 'asks every 5 s' : heldOpacity > 0.5 && f < b07.from ? 'keeps a request open' : prog(f, named[i] + 4, 10) > 0.5 ? what : null))}
        ring={{ node: 2, progress: ringProgress, opacity: ringOpacity }}
        held={{ opacity: heldOpacity, flow: f * 1.4 }}
      />

      {/* b04: the wait, counted */}
      <div style={{ position: 'absolute', left: nodeX(1) - 200, top: Y - R - 76, width: 400, textAlign: 'center', opacity: waitingOpacity }}>
        <Pill tone="accent" size={22}>{f < answered ? 'work waiting' : 'waited'} {waited.toFixed(1)} s</Pill>
      </div>
      {before >= 0 ? <Packet t={before} y={Y} opacity={beforeOpacity} pulse={f < answered && f > sent + 16 ? 0.5 + 0.5 * Math.sin((f - sent) / 4) : 0} /> : null}

      {/* b05: the held request and its clock */}
      <div style={{ position: 'absolute', left: (nodeX(1) + nodeX(2)) / 2 - 200, top: Y - 60, width: 400, textAlign: 'center', opacity: heldOpacity * (1 - prog(f, b06.from - 4, 8)) }}>
        <span style={{ ...body(23, C.blue, 500), fontVariantNumeric: 'tabular-nums' }}>held open · {countdown.toFixed(0)} s left</span>
      </div>
      {now >= 0 ? <Packet t={now} y={Y} opacity={nowOpacity} /> : null}

      {/* under the chain: before and now */}
      <div style={{ position: 'absolute', left: MX, top: 672, width: 1680, display: 'flex', gap: 60 }}>
        <Rise at={cue(b04, 'could sit')} out={statsOut} dim={{ at: arrives + 30, to: 0.5 }} style={{ flex: 1 }}>
          <Stat label="Before" value="up to 5 s" note="before anything you start remotely begins" color={C.ink2} />
        </Rise>
        <Rise at={arrives + 34} out={statsOut} style={{ flex: 1 }}>
          <Stat label="Now" value="at once" note="the account answers the request that is already open" color={C.accent} />
        </Rise>
      </div>

      {/* b06: where that half stands */}
      <Rise at={b06.from + 2} out={statusOut} style={{ position: 'absolute', left: MX, top: 690, width: 1680 }}>
        <Card pad="36px 44px" style={{ display: 'flex', alignItems: 'center', gap: 36 }}>
          <span style={{ width: 84, height: 84, borderRadius: 22, background: C.blueSoft, display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
            <Icon name="server" size={46} color={C.blue} />
          </span>
          <div style={{ flex: 1 }}>
            <div style={{ ...body(36, C.ink, 600), letterSpacing: '-0.015em' }}>The account server's half</div>
            <div style={{ ...body(26, C.muted), marginTop: 4 }}>Holds the open request and wakes it when work arrives.</div>
          </div>
          <div style={{ display: 'flex', gap: 18, alignItems: 'center' }}>
            <Rise at={cue(b06, 'written')} by={10}><Pill tone="ok" size={25}><Icon name="check" size={24} stroke={2.4} /> Written</Pill></Rise>
            <Rise at={cue(b06, 'tested')} by={10}><Pill tone="ok" size={25}><Icon name="check" size={24} stroke={2.4} /> Tested · 22 pass</Pill></Rise>
            <Rise at={cue(b06, "hasn't been deployed")} by={10}><Pill tone="warn" size={25} dot>Not deployed</Pill></Rise>
          </div>
        </Card>
      </Rise>

      {/* b07: the clock bro runs on until then */}
      <div style={{ position: 'absolute', left: MX, top: 664, width: 1680, display: 'flex', gap: 28 }}>
        {[
          { when: 'First 10 s after a job', every: '4 a second', rate: 4 },
          { when: 'Up to a minute', every: 'every 1 s', rate: 1 },
          { when: 'Up to 5 minutes', every: 'every 2 s', rate: 0.5 },
          { when: 'After that', every: 'every 5 s', rate: 0.2 },
        ].map((step, i) => (
          <Rise key={i} at={steps[i]} style={{ flex: 1 }}>
            <Card pad="24px 30px" tone={i === 0 ? 'raised' : 'plain'}>
              <Label>{step.when}</Label>
              <div style={{ fontSize: 56, fontWeight: 600, letterSpacing: '-0.03em', margin: '8px 0 14px', color: i === 0 ? C.accent : C.ink }}>{step.every}</div>
              <Tick perSecond={step.rate} from={steps[i] + 10} />
            </Card>
          </Rise>
        ))}
      </div>
      <Rise at={steps[3] + 14} style={{ position: 'absolute', left: MX, top: 902, width: 1680 }}>
        <div style={{ ...body(25, C.muted), display: 'flex', alignItems: 'center', gap: 12 }}>
          <Mark kind="ok" size={30} /> Once the server holds requests, bro stops using the clock by itself.
        </div>
      </Rise>

      {named.map((at, i) => <Sfx key={i} at={at} name="tick-bright" volume={0.07} />)}
      <Sfx at={answered} name="pop" volume={0.7} />
      <Sfx at={arrives} name="whoosh-short" volume={0.08} />
      <Sfx at={arrives + 40} name="pop" volume={0.7} />
    </>
  );
};
