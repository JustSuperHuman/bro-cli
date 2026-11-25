import React from 'react';
import { useCurrentFrame } from 'remotion';
import perf from '../data/perf.json';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Card, Header, Icon, Kbd, Label, Rise, SceneFC, Sfx, body, lin, mix, mono, prog } from '../ui';

const M = perf.before.measured;
/** What 2.1.49 has logged since it was installed. Absent fields mean the log had no such lines yet. */
type Since = { found: boolean; surface?: string | null; minutes?: number; graphics?: number; by?: Record<string, number>; sample?: { total: number; gpu: number; acquire: number; draw: number; present: number } };
const S = perf.since as Since;
/** A real line from before the split was read, for a log that has none of its own yet. */
const LINE = S.sample ?? { total: 63.8, gpu: 59.5, acquire: 2.9, draw: 1.0, present: 55.6 };
/** The step most waits on graphics were in, and how many. */
const TOP = Object.entries(S.by ?? {}).sort((a, b) => b[1] - a[1])[0] ?? ['present', 0];
/** The graphics API the real window draws through, as the log names it ("… RTX 4090 (Dx12)"). */
const API = (/\((\w+)\)\s*$/.exec(S.surface ?? '')?.[1] ?? '').replace(/^dx/i, 'DX');
const STEPS = [
  { name: 'acquire', does: 'get a frame to draw into' },
  { name: 'draw', does: 'draw it' },
  { name: 'present', does: 'hand it to the display' },
];
const COL = 410;

/** One measured run: where its worst frame spent its time. */
const Run: React.FC<{ name: string; note: string; step: string; ms: number; at: number; from?: string }> = ({ name, note, step, ms, at, from }) => {
  const f = useCurrentFrame();
  const to = STEPS.findIndex((s) => s.name === step);
  const start = from ? STEPS.findIndex((s) => s.name === from) : to;
  const moved = prog(f, at + 8, 22);
  const col = mix(start, to, moved);
  return (
    <div style={{ display: 'flex', alignItems: 'center', height: 92, opacity: prog(f, at, 12) }}>
      <div style={{ width: 330 }}>
        <div style={body(34, C.ink, 600)}>{name}</div>
        <div style={body(23, C.muted)}>{note}</div>
      </div>
      <div style={{ position: 'relative', flex: 1, height: 56 }}>
        {STEPS.map((s, i) => <div key={s.name} style={{ position: 'absolute', left: i * COL, top: 22, width: 46, height: 12, borderRadius: 6, background: 'rgba(255,255,255,0.12)' }} />)}
        <div style={{ position: 'absolute', left: col * COL, top: 0, width: 300, height: 56, borderRadius: 14, background: C.accent, display: 'flex', alignItems: 'center', padding: '0 20px', boxSizing: 'border-box', ...body(29, '#1b1206', 700), opacity: from ? mix(0.4, 1, moved) : 1 }}>
          {/* the number shows once the block has settled under its own step */}
          <span style={{ opacity: from ? prog(f, at + 26, 8) : 1 }}>{ms.toFixed(0)} ms</span>
        </div>
      </div>
    </div>
  );
};

export const Stall: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b16, b17, b18 } = scene.beats;
  const measured = cue(b16, 'We measured');
  const moves = cue(b16, 'switching only moves');

  const blocks = cue(b17, 'After a frame blocks');
  const holds = cue(b17, 'it stops drawing');
  const key = cue(b17, 'the next key');
  const blocked = lin(f, blocks, 18);
  const held = lin(f, holds, 22);
  const ended = prog(f, key + 4, 12);
  const resumed = lin(f, key + 8, 20);

  const TICK = 16;
  const blockX = 300;
  const blockW = 220;
  const holdX = blockX + blockW;
  const holdW = blockW * 4;
  const keyX = holdX + 440;
  const early = S.found && (S.graphics ?? 0) >= 5;

  return (
    <>
      <Header
        eyebrow="Speed"
        titles={[
          { at: 4, text: "The cause isn't found yet" },
          { at: b17.from - 4, text: 'So bro gets out of the way' },
          { at: b18.from - 4, text: 'And the log now names the step' },
        ]}
      />

      {/* b16: two ways to hand over a frame, the same stall */}
      <Rise at={measured - 8} dim={{ at: b17.from, to: 0.42 }} style={{ position: 'absolute', left: MX, top: 236, width: 1680 }}>
        <Card pad="24px 40px 22px">
          <div style={{ display: 'flex', alignItems: 'flex-end', height: 66 }}>
            <div style={{ width: 330 }}><Label>Worst frame</Label></div>
            {STEPS.map((s) => (
              <div key={s.name} style={{ width: COL }}>
                <div style={mono(27, C.ink)}>{s.name}</div>
                <div style={body(22, C.faint)}>{s.does}</div>
              </div>
            ))}
          </div>
          <div style={{ height: 1.5, background: C.line, margin: '14px 0 6px' }} />
          <Run name="Fifo" note="bro's default" step={M.fifo.step} ms={M.fifo.stepMs} at={measured} />
          <Run name="Mailbox" note="the other way" step={M.mailbox.step} ms={M.mailbox.stepMs} at={moves - 10} from={M.fifo.step} />
        </Card>
        <div style={{ ...body(22, C.faint), marginTop: 12 }}>
          Measured in bro's demo on Vulkan, debug build, about 1,500 frames per run. Outside that one step, each worst frame took under 2 ms.
        </div>
      </Rise>

      {/* b17: one blocked frame, then a hold that any input ends */}
      <Rise at={b17.from - 2} dim={{ at: b18.from, to: 0.42 }} style={{ position: 'absolute', left: MX, top: 622, width: 1680, height: 170 }}>
        <div style={{ position: 'absolute', left: 0, right: 0, top: 67, height: 2, background: C.line }} />
        {Array.from({ length: Math.floor(blockX / TICK) }).map((_, i) => (
          <div key={i} style={{ position: 'absolute', left: i * TICK, top: 54, width: 5, height: 28, borderRadius: 3, background: C.ok, opacity: lin(f, b17.from + 2 + i * 0.7, 4) * 0.85 }} />
        ))}
        <div style={{ position: 'absolute', left: blockX, top: 40, width: blockW * blocked, height: 56, borderRadius: 12, background: C.danger, overflow: 'hidden', display: 'flex', alignItems: 'center', boxSizing: 'border-box' }}>
          <span style={{ ...body(24, '#2a0b0b', 700), whiteSpace: 'nowrap', paddingLeft: 16 }}>blocked 2.5 s</span>
        </div>
        <div style={{ position: 'absolute', left: holdX + 6, top: 40, width: (holdW - 6) * held, height: 56, borderRadius: 12, overflow: 'hidden' }}>
          <div style={{ position: 'absolute', left: 0, top: 0, width: holdW - 6, height: 56, borderRadius: 12, border: `2px dashed rgba(232,194,104,0.6)`, background: C.warnSoft, boxSizing: 'border-box', display: 'flex', alignItems: 'center' }}>
            <span style={{ ...body(24, C.warn, 600), whiteSpace: 'nowrap', paddingLeft: 18 }}>no drawing: held 10 s</span>
          </div>
          <div style={{ position: 'absolute', left: keyX - holdX - 6, top: 0, right: 0, bottom: 0, background: C.bg, opacity: ended * 0.86 }} />
        </div>
        {Array.from({ length: Math.floor((1680 - keyX) / TICK) }).map((_, i) => (
          <div key={i} style={{ position: 'absolute', left: keyX + 10 + i * TICK, top: 54, width: 5, height: 28, borderRadius: 3, background: C.ok, opacity: (resumed * 46 > i ? 0.85 : 0) }} />
        ))}
        <div style={{ position: 'absolute', left: keyX - 30, top: mix(-46, -14, prog(f, key, 10)), opacity: prog(f, key, 8) }}>
          <Kbd size={26}>k</Kbd>
        </div>
        <div style={{ position: 'absolute', left: keyX - 1, top: 34, width: 3, height: 68, borderRadius: 2, background: C.ink, opacity: prog(f, key + 2, 8) }} />
        <Rise at={holds + 12} by={8} style={{ position: 'absolute', left: holdX + 6, top: 116 }}>
          <span style={body(24, C.muted)}>Four times as long as the stall, half a minute at most.</span>
        </Rise>
        <Rise at={key + 10} by={8} style={{ position: 'absolute', right: 0, top: 116 }}>
          <span style={body(24, C.ink, 500)}>A key or pointer move ends it.</span>
        </Rise>
      </Rise>

      {/* b18: the log line, as 2.1.49 writes it */}
      <Rise at={b18.from - 2} style={{ position: 'absolute', left: MX, top: 800, width: 1680 }}>
        <Card tone="raised" pad="22px 32px" style={{ display: 'flex', alignItems: 'center', gap: 26 }}>
          <Icon name="terminal" size={40} color={C.muted} />
          <div style={{ flex: 1 }}>
            <div style={{ ...mono(25, C.ink2), whiteSpace: 'nowrap' }}>
              slow pass {LINE.total.toFixed(1)} ms · gpu {LINE.gpu.toFixed(1)} ms{' '}
              <span style={{ color: C.accent, background: C.accentSoft, borderRadius: 8, padding: '4px 10px' }}>
                (acquire {LINE.acquire.toFixed(1)} ms · draw {LINE.draw.toFixed(1)} ms · present {LINE.present.toFixed(1)} ms)
              </span>
            </div>
            <div style={{ ...body(25, C.muted), marginTop: 10 }}>
              {early ? (
                <>
                  Your window{API ? ` (${API})` : ''}, first {S.minutes} minutes on 2.1.49: <span style={{ color: C.ink, fontWeight: 600 }}>{TOP[1]} of {S.graphics}</span> waits on graphics were in {TOP[0]}. Too few to call yet.
                </>
              ) : (
                <>A real line from the log. A day of them says where the long stalls sit.</>
              )}
            </div>
          </div>
        </Card>
      </Rise>

      <Sfx at={moves} name="whoosh-short" volume={0.07} />
      <Sfx at={blocks} name="impact-soft" volume={0.07} />
      <Sfx at={key} name="tick-bright" volume={0.09} />
    </>
  );
};
