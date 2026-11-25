import React from 'react';
import { useCurrentFrame } from 'remotion';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Card, Header, Icon, Label, Pill, Rise, SceneFC, Sfx, body, mix, mono, prog } from '../ui';

const FileChip: React.FC<{ name: string; note: string; tone?: 'plain' | 'ok' | 'danger'; struck?: number }> = ({ name, note, tone = 'plain', struck = 0 }) => {
  const ink = tone === 'ok' ? C.ok : tone === 'danger' ? C.danger : C.ink2;
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 14, height: 64, padding: '0 20px', borderRadius: 14, border: `1.5px solid ${tone === 'ok' ? 'rgba(143,207,156,0.4)' : tone === 'danger' ? 'rgba(241,126,126,0.35)' : C.line}`, background: tone === 'ok' ? C.okSoft : tone === 'danger' ? C.dangerSoft : C.well, boxSizing: 'border-box' }}>
      <Icon name="file" size={28} color={ink} />
      <span style={{ position: 'relative', ...mono(23, ink) }}>
        {name}
        <span style={{ position: 'absolute', left: 0, right: 0, top: '52%', height: 2, background: ink, transformOrigin: 'left center', transform: `scaleX(${struck})` }} />
      </span>
      <span style={{ ...body(22, C.muted), marginLeft: 6, whiteSpace: 'nowrap' }}>{note}</span>
    </div>
  );
};

export const Small: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b19, b20 } = scene.beats;
  const was = cue(b19, 'used to be replaced');
  const kept = cue(b19, 'Now the old file');
  const told = cue(b19, 'bro tells you');
  const gone = cue(b20, 'no longer stays');
  const left = prog(f, gone, 14);
  // pings go out every 30 s; here one crosses every second and a half so the rhythm can be seen
  const ping = ((Math.max(0, f - b20.from) % 45) / 45);

  return (
    <>
      <Header eyebrow="Small things" titles={[{ at: 4, text: 'Two smaller fixes' }]} />

      <Rise at={b19.from - 4} dim={{ at: b20.from, to: 0.5 }} style={{ position: 'absolute', left: MX, top: 246, width: 810 }}>
        <Card pad="34px 38px" style={{ height: 660 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <div style={{ ...body(32, C.ink, 600), letterSpacing: '-0.015em' }}>A settings file bro can't read</div>
            <Pill tone="warn" size={21}><Icon name="alert" size={22} /> v2.toml</Pill>
          </div>

          <Rise at={was - 4} style={{ marginTop: 28 }}>
            <Label>Before</Label>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 14 }}>
              <FileChip name="v2.toml" note="your settings: overwritten at your next change" tone="danger" struck={prog(f, was + 10, 16)} />
              <FileChip name="v2.toml" note="defaults, written in its place" />
            </div>
          </Rise>

          <Rise at={kept - 2} style={{ marginTop: 26 }}>
            <Label>Now</Label>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 14 }}>
              <FileChip name="v2.toml.unreadable-20261110-120000" note="your file, kept" tone="ok" />
              <FileChip name="v2.toml" note="the new one, beside it" />
            </div>
          </Rise>

          <Rise at={told} style={{ marginTop: 26 }}>
            <div style={{ display: 'flex', gap: 16, padding: '18px 20px', borderRadius: 14, background: C.warnSoft, border: `1.5px solid rgba(232,194,104,0.35)` }}>
              <Icon name="alert" size={28} color={C.warn} style={{ marginTop: 3 }} />
              <div style={body(23, C.ink2)}>
                <span style={{ color: C.muted }}>At startup: </span>
                v2.toml couldn't be read. Your file is untouched, and if you change a setting it is kept beside the new one.
              </div>
            </div>
          </Rise>
        </Card>
      </Rise>

      <Rise at={b20.from - 4} style={{ position: 'absolute', left: MX + 870, top: 246, width: 810 }}>
        <Card tone="raised" pad="34px 38px" style={{ height: 660 }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <div style={{ ...body(32, C.ink, 600), letterSpacing: '-0.015em' }}>A phone that left without saying so</div>
          </div>

          <div style={{ position: 'relative', height: 190, marginTop: 30 }}>
            <div style={{ position: 'absolute', left: 30, top: 28, width: 120, height: 120, borderRadius: 30, background: C.well, border: `1.5px solid ${C.line}`, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
              <Icon name="bro" size={76} />
            </div>
            <div style={{ position: 'absolute', right: 30, top: 28, width: 120, height: 120, borderRadius: 30, background: C.well, border: `1.5px solid ${C.line}`, display: 'flex', alignItems: 'center', justifyContent: 'center', opacity: mix(1, 0.35, left) }}>
              <Icon name="phone" size={64} color={C.ink2} />
            </div>
            <div style={{ position: 'absolute', left: 170, right: 170, top: 87, height: 2, background: left > 0.5 ? C.line : C.lineStrong }} />
            <div style={{ position: 'absolute', left: 170 + ping * (810 - 76 - 340 - 18), top: 79, width: 18, height: 18, borderRadius: 999, background: C.blue, opacity: f > b20.from ? 1 - ping * 0.5 : 0, boxShadow: '0 0 20px rgba(127,178,255,0.7)' }} />
            <div style={{ position: 'absolute', left: 0, right: 0, top: 30, textAlign: 'center', ...body(23, C.blue, 500) }}>a ping every 30 s</div>
            <div style={{ position: 'absolute', left: 0, right: 0, top: 112, textAlign: 'center', ...body(22, C.muted) }}>{left > 0.5 ? 'nothing comes back' : 'the phone answers each one'}</div>
            <div style={{ position: 'absolute', left: 0, right: 0, top: 160, display: 'flex', justifyContent: 'center' }}>
              <div style={{ position: 'relative', width: 220, height: 44 }}>
                <div style={{ position: 'absolute', inset: 0, display: 'flex', justifyContent: 'center', opacity: 1 - left }}><Pill tone="ok" size={22} dot>1 viewer</Pill></div>
                <div style={{ position: 'absolute', inset: 0, display: 'flex', justifyContent: 'center', opacity: left }}><Pill size={22} dot>0 viewers</Pill></div>
              </div>
            </div>
          </div>

          <div style={{ height: 1.5, background: C.line, margin: '34px 0 26px' }} />
          <Label>How soon bro notices</Label>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 14, marginTop: 16 }}>
            {[
              { what: 'Network gone', when: 'in about a minute', at: gone - 6 },
              { what: 'App frozen in the background', when: 'after 10 minutes of silence', at: gone + 4 },
            ].map((row) => (
              <Rise key={row.what} at={row.at} by={12} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', height: 64, padding: '0 22px', borderRadius: 14, background: C.well, border: `1.5px solid ${C.line}` }}>
                <span style={{ ...body(25, C.ink2), whiteSpace: 'nowrap' }}>{row.what}</span>
                <span style={{ ...body(25, C.ink, 600), whiteSpace: 'nowrap' }}>{row.when}</span>
              </Rise>
            ))}
          </div>
        </Card>
      </Rise>

      <Sfx at={kept} name="pop" volume={0.6} />
      <Sfx at={gone} name="tick-bright" volume={0.07} />
    </>
  );
};
