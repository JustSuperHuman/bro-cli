import React from 'react';
import { C } from './theme';
import { Icon, body } from './ui';

/** The four links a remote request crosses, in order. */
export const NODES = [
  { name: 'This computer', icon: 'laptop' },
  { name: 'Just Superhuman', icon: 'knot' },
  { name: 'The other computer', icon: 'monitor' },
  { name: 'Bro there', icon: 'bro' },
] as const;

export const nodeX = (i: number) => 330 + i * 420;
export const R = 80;

export type ChainProps = {
  y: number;
  /** How bright each link is, 0 to 1. */
  lit: number[];
  /** How much of each connector is drawn, 0 to 1. */
  link: number[];
  /** A line under a link's name. */
  sub?: (string | null)[];
  /** A ring around one link that fills as time passes (the other computer's "ask again" clock). */
  ring?: { node: number; progress: number; opacity: number };
  /** The connector from the other computer to the account, drawn as a request held open. */
  held?: { opacity: number; flow: number };
};

export const Chain: React.FC<ChainProps> = ({ y, lit, link, sub, ring, held }) => (
  <div style={{ position: 'absolute', left: 0, top: 0, width: '100%', height: '100%' }}>
    {[0, 1, 2].map((i) => {
      const x0 = nodeX(i) + R + 16;
      const x1 = nodeX(i + 1) - R - 16;
      const heldHere = i === 1 && held ? held.opacity : 0;
      return (
        <React.Fragment key={i}>
          <div style={{ position: 'absolute', left: x0, top: y - 1, width: (x1 - x0) * link[i], height: 2, borderRadius: 2, background: C.lineStrong, opacity: 1 - heldHere }} />
          {heldHere > 0 ? (
            <svg width={x1 - x0} height={24} style={{ position: 'absolute', left: x0, top: y - 12, opacity: heldHere }}>
              <line x1={0} y1={12} x2={x1 - x0} y2={12} stroke={C.blue} strokeOpacity={0.22} strokeWidth={12} strokeLinecap="round" />
              <line x1={4} y1={12} x2={x1 - x0 - 4} y2={12} stroke={C.blue} strokeWidth={3} strokeLinecap="round" strokeDasharray="14 12" strokeDashoffset={held ? held.flow : 0} />
            </svg>
          ) : null}
        </React.Fragment>
      );
    })}
    {NODES.map((node, i) => {
      const on = lit[i];
      const ringOn = ring && ring.node === i ? ring.opacity : 0;
      const circumference = 2 * Math.PI * (R + 11);
      return (
        <React.Fragment key={i}>
          {ringOn > 0 && ring ? (
            <svg width={(R + 16) * 2} height={(R + 16) * 2} style={{ position: 'absolute', left: nodeX(i) - R - 16, top: y - R - 16, opacity: ringOn, transform: 'rotate(-90deg)' }}>
              <circle cx={R + 16} cy={R + 16} r={R + 11} fill="none" stroke="rgba(255,255,255,0.08)" strokeWidth={5} />
              <circle cx={R + 16} cy={R + 16} r={R + 11} fill="none" stroke={C.accent} strokeWidth={5} strokeLinecap="round" strokeDasharray={`${circumference * ring.progress} ${circumference}`} />
            </svg>
          ) : null}
          <div style={{ position: 'absolute', left: nodeX(i) - R, top: y - R, width: R * 2, height: R * 2, borderRadius: 999, boxSizing: 'border-box', background: C.panel2, border: `1.5px solid rgba(255,255,255,${0.1 + 0.16 * on})`, display: 'flex', alignItems: 'center', justifyContent: 'center', opacity: 0.4 + 0.6 * on }}>
            <Icon name={node.icon} size={node.icon === 'bro' ? 86 : 64} color={i === 1 ? C.blue : C.ink} stroke={1.6} />
          </div>
          <div style={{ position: 'absolute', left: nodeX(i) - 200, top: y + R + 24, width: 400, textAlign: 'center', opacity: 0.4 + 0.6 * on }}>
            <div style={{ ...body(33, C.ink, 600), letterSpacing: '-0.015em' }}>{node.name}</div>
            {sub?.[i] ? <div style={{ ...body(24, C.muted), marginTop: 4 }}>{sub[i]}</div> : null}
          </div>
        </React.Fragment>
      );
    })}
  </div>
);

/** A piece of work on its way along the chain. `t` runs 0 (this computer) to 3 (Bro there). */
export const Packet: React.FC<{ t: number; y: number; opacity?: number; pulse?: number }> = ({ t, y, opacity = 1, pulse = 0 }) => {
  const size = 24 + pulse * 8;
  return (
    <div style={{ position: 'absolute', left: nodeX(0) + t * 420 - size / 2, top: y - size / 2, width: size, height: size, borderRadius: 999, background: C.accent, boxShadow: `0 0 0 ${6 + pulse * 10}px rgba(246,160,77,${0.16 + pulse * 0.1}), 0 0 34px rgba(246,160,77,0.55)`, opacity }} />
  );
};
