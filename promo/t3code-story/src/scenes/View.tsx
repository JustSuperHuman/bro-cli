import React from 'react';
import { Img, staticFile, useCurrentFrame } from 'remotion';
import { C, MX } from '../theme';
import { cue } from '../timeline';
import { Header, Icon, Kbd, Rise, SceneFC, Sfx, body, mix, prog } from '../ui';

/** The window the screenshots sit in, and the shots' own size. */
const BOX = { x: MX, y: 232, w: 1680, h: 706 };
const SHOT = { w: 1440, h: 931 };

/** A look at the screenshot: how much it is enlarged, and which of its points sits in the middle of the window. */
type Look = { s: number; cx: number; cy: number };
const WHOLE: Look = { s: BOX.w / SHOT.w, cx: SHOT.w / 2, cy: BOX.h / 2 / (BOX.w / SHOT.w) };
const PANEL: Look = { s: 2.05, cx: 1032, cy: 304 };
const CLOSED: Look = { s: 2.05, cx: 1032, cy: 336 };

const between = (a: Look, b: Look, t: number): Look => ({ s: mix(a.s, b.s, t), cx: mix(a.cx, b.cx, t), cy: mix(a.cy, b.cy, t) });
const place = (v: Look) => ({ tx: BOX.w / 2 - v.s * v.cx, ty: BOX.h / 2 - v.s * v.cy });
/** A rectangle of the screenshot, in window coordinates under a look. */
const rect = (v: Look, r: [number, number, number, number]) => {
  const { tx, ty } = place(v);
  return { left: v.s * r[0] + tx, top: v.s * r[1] + ty, width: v.s * (r[2] - r[0]), height: v.s * (r[3] - r[1]) };
};

const QUERY = 'computers';

export const View: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const { b12, b13 } = scene.beats;
  const typing = b12.from + 22;
  const typed = Math.max(0, Math.min(QUERY.length, Math.floor((f - typing) / 2.2)));
  const found = typing + Math.ceil(QUERY.length * 2.2) + 4;
  const paletteOut = b13.from - 14;

  const shown = b13.from - 8;
  const zoom = cue(b13, 'is listed');
  const offers = cue(b13, 'only offers');
  const swap = b13.from + b13.frames + 16;
  const zoomed = prog(f, zoom, 30);
  const swapped = prog(f, swap, 18);
  const look = between(between(WHOLE, PANEL, zoomed), CLOSED, swapped);
  const { tx, ty } = place(look);
  const ringA = rect(look, [1068, 168, 1378, 213]);
  const ringB = rect(look, [1228, 168, 1378, 213]);
  const ring = { left: mix(ringA.left, ringB.left, swapped), top: mix(ringA.top, ringB.top, swapped), width: mix(ringA.width, ringB.width, swapped), height: mix(ringA.height, ringB.height, swapped) };
  const ringOn = prog(f, offers, 12);

  return (
    <>
      <Header eyebrow="Remote computers" titles={[{ at: 4, text: 'One place to see all of it' }]} />

      {/* b12: the palette */}
      <Rise at={b12.from - 2} out={paletteOut} style={{ position: 'absolute', left: (1920 - 1040) / 2, top: 330, width: 1040 }}>
        <div style={{ background: C.panel2, border: `1.5px solid ${C.lineStrong}`, borderRadius: 26, boxShadow: '0 40px 100px rgba(0,0,0,0.5)', overflow: 'hidden' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 20, height: 104, padding: '0 34px', borderBottom: `1.5px solid ${C.line}` }}>
            <Icon name="search" size={38} color={C.muted} />
            <div style={{ ...body(40, C.ink, 500), display: 'flex', alignItems: 'center' }}>
              {QUERY.slice(0, typed)}
              <span style={{ display: 'inline-block', width: 3, height: 44, marginLeft: 3, background: C.accent, opacity: Math.floor(f / 9) % 2 === 0 ? 1 : 0.15 }} />
              {typed === 0 ? <span style={{ color: C.faint, marginLeft: 8 }}>Search bro…</span> : null}
            </div>
          </div>
          <div style={{ padding: 18, minHeight: 132 }}>
            <Rise at={found} by={10} dur={10}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 24, padding: '20px 22px', borderRadius: 18, background: 'rgba(255,255,255,0.07)' }}>
                <span style={{ width: 62, height: 62, borderRadius: 16, background: C.accentSoft, display: 'flex', alignItems: 'center', justifyContent: 'center', flex: 'none' }}>
                  <Icon name="monitor" size={34} color={C.accent} />
                </span>
                <div style={{ flex: 1 }}>
                  <div style={{ ...body(34, C.ink, 600), letterSpacing: '-0.015em' }}>Computers</div>
                  <div style={body(25, C.muted)}>Every computer on your account, and the sessions on each</div>
                </div>
                <Kbd>enter</Kbd>
              </div>
            </Rise>
          </div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 12, marginTop: 30, ...body(26, C.muted) }}>
          <Kbd>alt</Kbd>
          <Kbd>p</Kbd>
          <span style={{ marginLeft: 8 }}>opens the palette</span>
        </div>
      </Rise>

      {/* b13: the view itself, from screenshots of bro's demo */}
      <Rise at={shown} by={30} dur={20} style={{ position: 'absolute', left: BOX.x, top: BOX.y, width: BOX.w, height: BOX.h }}>
        <div style={{ position: 'absolute', inset: 0, borderRadius: 22, overflow: 'hidden', border: `1.5px solid ${C.lineStrong}`, boxShadow: '0 40px 110px rgba(0,0,0,0.55)', background: '#0e1015' }}>
          <div style={{ position: 'absolute', left: 0, top: 0, width: SHOT.w, height: SHOT.h, transformOrigin: '0 0', transform: `translate(${tx}px, ${ty}px) scale(${look.s})` }}>
            <Img src={staticFile('img/computers-sessions.png')} style={{ position: 'absolute', left: 0, top: 0, width: SHOT.w, height: SHOT.h }} />
            <Img src={staticFile('img/computers-closed.png')} style={{ position: 'absolute', left: 0, top: 0, width: SHOT.w, height: SHOT.h, opacity: swapped }} />
          </div>
          <div style={{ position: 'absolute', ...ring, borderRadius: 16, border: `3px solid ${C.accent}`, boxShadow: '0 0 0 6px rgba(246,160,77,0.18)', boxSizing: 'border-box', opacity: ringOn }} />
          <div style={{ position: 'absolute', right: BOX.w - (ring.left + ring.width), top: ring.top + ring.height + 16, opacity: ringOn }}>
            <div style={{ position: 'relative', height: 58 }}>
              {[
                { text: 'Ready, so both actions are offered', on: 1 - swapped },
                { text: 'Bro is closed there, so only its desktop is offered', on: swapped },
              ].map((label) => (
                <div key={label.text} style={{ position: 'absolute', right: 0, top: 0, whiteSpace: 'nowrap', height: 58, padding: '0 24px', display: 'flex', alignItems: 'center', borderRadius: 14, background: '#0b0d11', border: `1.5px solid ${C.accentLine}`, ...body(26, C.ink, 500), opacity: label.on }}>
                  {label.text}
                </div>
              ))}
            </div>
          </div>
        </div>
      </Rise>

      <Sfx at={found} name="pop" volume={0.6} />
      <Sfx at={shown} name="whoosh-short" volume={0.07} />
      <Sfx at={swap} name="tick-bright" volume={0.07} />
    </>
  );
};
