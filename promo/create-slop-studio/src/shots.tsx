// The shot list: what each scene shows, in order, and when. Frames are from the start of the scene; `cue(scene, line, 'phrase')`
// is the frame a phrase is spoken at, so what appears stays with the words. Boxes are in the capture's own pixels
// (the picture bro made, before it went onto the desktop): the main captures and the time-lapse are 1760 wide, the
// toolkit capture and the storyboard close-ups ("cu-*") are 1440 wide and use SMALL_PIC.
import React from 'react';
import { AbsoluteFill, Audio, Sequence, staticFile, useCurrentFrame } from 'remotion';
import assets from './data/assets.json';
import { ClipShot, PullBack, clipSeconds, timelapse } from './clips';
import { Box, Desk, Layer, Move, Pointer, Pt, Ring, SMALL_PIC } from './desk';
import { EndCard } from './endcard';
import { FPS } from './theme';
import { Scene, at, cue, sceneOf } from './timeline';
import { lin } from './ui';

export type SceneFC = React.FC<{ scene: Scene }>;

/** At most `n` frames of the time-lapse, evenly spread, first and last kept. */
const sample = (all: string[], n: number) => (all.length <= n ? all : Array.from({ length: n }, (_, i) => all[Math.round((i * (all.length - 1)) / (n - 1))]));
const TL = sample(assets.tl as string[], 70);
/** The last frames of the time-lapse: takes landing on the board, the timeline blocks turning green. */
const TAKES = (assets.tl as string[]).slice(-14);
/** The last time-lapse frame: the studio with the board as far as it has got. */
export const BOARD = assets.tlLast as string;

const sfx = (name: string, from: number, volume = 0.5) => (
  <Sequence from={Math.max(0, from)} durationInFrames={45} layout="none">
    <Audio src={staticFile(`sfx/${name}.wav`)} volume={volume} />
  </Sequence>
);

/** Shows its children from frame `from` of the scene (their own frames count from there), fading in. */
const Sub: React.FC<{ from: number; to?: number; fade?: number; children: React.ReactNode }> = ({ from, to, fade = 7, children }) => {
  const f = useCurrentFrame();
  return (
    <Sequence from={from} durationInFrames={to === undefined ? undefined : to - from} layout="none">
      <AbsoluteFill style={{ opacity: Math.min(lin(f - from, 0, fade), to === undefined ? 1 : 1 - lin(f - to, -fade, fade)) }}>{children}</AbsoluteFill>
    </Sequence>
  );
};

// ---- the places on the page, the board and the close-ups (capture px) -------------------------------------------------
export const BOXES = {
  // 1760-wide captures
  tab: [270, 113, 123, 29] as Box,
  tabPage: [20, 100, 760, 170] as Box,
  castLeft: [20, 150, 790, 345] as Box,
  toolkit: [1325, 165, 410, 520] as Box,
  keyRow: [1328, 300, 405, 215] as Box,
  startButton: [1300, 858, 440, 80] as Box,
  sessionTop: [10, 45, 740, 500] as Box,
  boardTop: [750, 45, 1000, 500] as Box,
  preview: [1091, 335, 360, 202.5] as Box,
  strip: [775, 195, 950, 50] as Box,
  // 1440-wide toolkit capture
  kitTop: [1005, 195, 395, 125] as Box,
  kitInstall: [1005, 520, 395, 64] as Box,
  kitInstallButton: [1322, 536, 76, 32] as Box,
  // 1440-wide storyboard close-ups
  list: [40, 267, 330, 330] as Box,
  script: [355, 636, 820, 56] as Box,
  scriptAndDirection: [355, 636, 820, 240] as Box,
  direction: [355, 732, 820, 140] as Box,
  approveHead: [355, 268, 820, 80] as Box,
  approveButton: [1074, 268, 101, 31] as Box,
  composerArea: [355, 560, 820, 320] as Box,
  composer: [355, 810, 820, 56] as Box,
  // the time-lapse picture where my request shows as a message and scene 7 is marked (1760-wide capture)
  message: [140, 540, 585, 160] as Box,
  scene7: [775, 700, 290, 100] as Box,
  keyframes: [355, 335, 870, 265] as Box,
};

// ---- cold open: the film, full frame; the song stops on 'Pause.' and the picture holds ---------------------------------
/** Song seconds at which the music stops: right after Meta Muse's 'Hi, viewers!'. */
export const PAUSE_AT = 28.4;
export const Cold: SceneFC = ({ scene }) => {
  const f = useCurrentFrame();
  const pauseFrame = Math.round((PAUSE_AT - assets.clips.cold.songAt) * FPS);
  const frozen = lin(f, pauseFrame, 10);
  return (
    <AbsoluteFill style={{ filter: `saturate(${1 - 0.45 * frozen}) brightness(${1 - 0.18 * frozen})` }}>
      <ClipShot name="cold" total={scene.frames} push={0.02} />
    </AbsoluteFill>
  );
};

// ---- pull back into the storyboard, then to the Create Slop page --------------------------------------------------------
export const Pull: SceneFC = ({ scene }) => {
  const cold = sceneOf('cold');
  const freeze = Math.max(0, Math.min(cold.frames / FPS, clipSeconds('cold') - 0.12));
  const mode = cue(scene, 'n02', 'Create Slop mode') - 14;
  const moves: Move[] = [{ at: 12, box: BOXES.tabPage, zmax: 1.7 }];
  return (
    <>
      <PullBack clip="cold" freezeAt={freeze} picture={BOARD} box={BOXES.preview} at={4} dur={46} />
      {sfx('whoosh-short', 2, 0.6)}
      <Sub from={mode} fade={10}>
        <Desk layers={[{ src: 'page', at: 0 }]} moves={moves} total={scene.frames - mode} overlay={<Ring box={BOXES.tab} at={14} pad={7} radius={9} />} />
      </Sub>
    </>
  );
};

// ---- pick the cast -----------------------------------------------------------------------------------------------------
export const Cast: SceneFC = ({ scene }) => {
  const bone = cue(scene, 'b04', 'Chinese Bone');
  const claudia = cue(scene, 'b04', 'Claudia');
  const pts: Pt[] = [
    { at: bone - 14, x: 480, y: 372 },
    { at: bone - 3, x: 482, y: 376, click: true },
    { at: claudia - 4, x: 124, y: 372, click: true },
  ];
  const layers: Layer[] = [
    { src: 'page', at: 0 },
    { src: 'hover-bone', at: pts[0].at + 2, fade: 4 },
    { src: 'bone', at: pts[1].at + 1, fade: 4 },
    { src: 'both', at: pts[2].at + 1, fade: 4 },
  ];
  const moves: Move[] = [{ at: 2, box: BOXES.castLeft, zmax: 1.9 }];
  return <Desk layers={layers} moves={moves} total={scene.frames} overlay={<Pointer pts={pts} />} />;
};

// ---- the tools --------------------------------------------------------------------------------------------------------
export const Tools: SceneFC = ({ scene }) => {
  const install = at(scene, 'b05b');
  const key = at(scene, 'b05c');
  const typed = cue(scene, 'b05c', 'it once');
  const names = ['t-justimagine', 't-elevenlabs', 't-higgsfield', 't-slopcannon', 't-ffmpeg', 't-remotion'];
  const per = Math.round((at(scene, 'b05a', 62) - 8) / names.length);
  const logos: Layer[] = names.map((src, i) => ({ src, at: i === 0 ? 0 : 6 + i * per }));
  const kitFrom = install - 18;
  const click = cue(scene, 'b05b', 'installs') + 4;
  const kitPts: Pt[] = [{ at: click - kitFrom, x: 1360, y: 552, click: true }];
  const keyLayers: Layer[] = [
    { src: 't-higgsfield', at: 0 },
    { src: 'key-field', at: 14 },
    { src: 'key-typed', at: typed - key + 4 },
  ];
  const keyPts: Pt[] = [{ at: 18, x: 1440, y: 468, click: true }];
  return (
    <>
      <Desk layers={logos} moves={[{ at: 2, box: BOXES.toolkit }]} total={kitFrom} />
      {/* the real toolkit, with its Start / Add key / Install buttons: only the panel is ever in frame */}
      <Sub from={kitFrom} to={key - 4}>
        <Desk
          pic={SMALL_PIC}
          layers={[{ src: 'kit', at: 0 }]}
          moves={[
            { at: 0, box: BOXES.kitTop, zmax: 3.5 },
            { at: click - kitFrom - 70, box: BOXES.kitInstall, zmax: 3.5 },
          ]}
          drift={0}
          total={key - kitFrom}
          overlay={<Pointer pts={kitPts} />}
        />
      </Sub>
      <Sub from={key - 8}>
        <Desk layers={keyLayers} moves={[{ at: 0, box: BOXES.keyRow, zmax: 2.2 }]} total={scene.frames - key} overlay={<Pointer pts={keyPts} />} />
      </Sub>
    </>
  );
};

// ---- start cooking: the agent takes one side, the film the other --------------------------------------------------------
export const Start: SceneFC = ({ scene }) => {
  const click = at(scene, 'b06', 26);
  const agent = cue(scene, 'b06', 'The agent');
  const film = cue(scene, 'b06', 'The film');
  const lapse = timelapse(TL, click + 4, scene.frames - 16);
  const layers: Layer[] = [{ src: 'start-hover', at: 0 }, { ...lapse[0], at: click + 4, fade: 6 }, ...lapse.slice(1)];
  const moves: Move[] = [
    { at: 2, box: BOXES.startButton, zmax: 2.3 },
    { at: agent - 8, box: BOXES.sessionTop, zmax: 1.4 },
    { at: film - 8, box: BOXES.boardTop, zmax: 1.4 },
  ];
  const pts: Pt[] = [{ at: click, x: 1555, y: 895, click: true }];
  return <Desk layers={layers} moves={moves} total={scene.frames} overlay={<Pointer pts={pts} />} />;
};

// ---- it writes every scene: what you hear, what you see ------------------------------------------------------------------
export const Board: SceneFC = ({ scene }) => {
  const hear = cue(scene, 'n06', 'what you hear');
  const see = cue(scene, 'n06', 'what you see');
  const moves: Move[] = [
    { at: 0, box: BOXES.list, zmax: 1.8 },
    { at: hear - 16, box: BOXES.scriptAndDirection, zmax: 1.8 },
  ];
  return (
    <Desk
      pic={SMALL_PIC}
      layers={[{ src: 'cu-p01-plan', at: 0 }]}
      moves={moves}
      total={scene.frames}
      overlay={
        <>
          <Ring box={BOXES.script} at={hear} out={see - 2} />
          <Ring box={BOXES.direction} at={see} out={see + 70} />
        </>
      }
    />
  );
};

// ---- approve what you like ------------------------------------------------------------------------------------------------
export const Approve: SceneFC = ({ scene }) => {
  const click = cue(scene, 'n07', 'Approve') + 14;
  const layers: Layer[] = [
    { src: 'cu-p01-plan', at: 0 },
    { src: 'cu-p02-approve-hover', at: click - 10, fade: 3 },
    { src: 'cu-p03-approving', at: click + 1, fade: 3 },
    { src: 'cu-p04-approved', at: click + 12, fade: 5 },
  ];
  const pts: Pt[] = [{ at: click - 2, x: 1125, y: 284, click: true }];
  return <Desk pic={SMALL_PIC} layers={layers} moves={[{ at: 0, box: BOXES.approveHead, zmax: 1.7 }]} total={scene.frames} overlay={<Pointer pts={pts} />} />;
};

// ---- ask for something dumber: the real request, typed ---------------------------------------------------------------------
const ASK_FRAMES = (assets.cu as string[]).filter((n) => /^cu-q\d\d-ask$/.test(n)).sort();
export const Ask: SceneFC = ({ scene }) => {
  const dumb = at(scene, 'n08a');
  const send = at(scene, 'b09b');
  const typeStart = dumb + scene.beats[0].frames + 12;
  // the close-ups are played at 12 a second
  const typeEnd = typeStart + Math.round((ASK_FRAMES.length / 12) * FPS);
  const sent = cue(scene, 'b09b', 'Send it') + 2;
  const toBoard = cue(scene, 'b09b', 'the agent') + 6;
  const typing = ASK_FRAMES.map((src, i) => ({ src, at: Math.round(typeStart + ((typeEnd - typeStart) * i) / Math.max(1, ASK_FRAMES.length)), fade: 1 }));
  const layers: Layer[] = [{ src: 'cu-p05-scene-7', at: 0 }, ...typing, { src: 'cu-q30-asked', at: sent, fade: 2 }];
  const moves: Move[] = [{ at: dumb + 6, box: BOXES.composerArea, zmax: 1.8 }];
  void send;
  return (
    <>
      <Desk pic={SMALL_PIC} layers={layers} moves={moves} total={scene.frames} overlay={<Ring box={BOXES.composer} at={typeStart} out={sent} pad={5} radius={6} />} />
      {/* the same request, arriving in the agent's conversation and marked on the board (the real time-lapse) */}
      <Sub from={toBoard} fade={5}>
        <Desk
          layers={[{ src: 'tl-040', at: 0 }]}
          moves={[
            { at: 0, box: BOXES.message, zmax: 1.7 },
            { at: 46, box: BOXES.scene7, zmax: 2.2 },
          ]}
          drift={0}
          total={scene.frames - toBoard}
          overlay={<Ring box={BOXES.message} at={6} out={44} pad={6} radius={8} />}
        />
      </Sub>
    </>
  );
};

// ---- it draws the keyframes, you pick -------------------------------------------------------------------------------------
export const Keyframes: SceneFC = ({ scene }) => {
  const pick = cue(scene, 'n09', 'You pick');
  const layers: Layer[] = [
    { src: 'cu-k01-scene-4', at: 0 },
    { src: 'cu-k02-hover-b', at: pick - 20, fade: 4 },
    { src: 'cu-k03-picking', at: pick + 2, fade: 3 },
    { src: 'cu-k04-picked-b', at: pick + 14, fade: 4 },
  ];
  const pts: Pt[] = [
    { at: pick - 22, x: 960, y: 384 },
    { at: pick + 2, x: 960, y: 384, click: true },
  ];
  return <Desk pic={SMALL_PIC} layers={layers} moves={[{ at: 0, box: BOXES.keyframes, zmax: 1.6 }]} total={scene.frames} overlay={<Pointer pts={pts} />} />;
};

// ---- it films them, lip-synced to the song -> the alligator -------------------------------------------------------------------
/** Frame (from the scene's start) the film cuts to the alligator take. */
export const cutToGator = (scene: Scene) => at(scene, 'n10b') - 16;

export const Film: SceneFC = ({ scene }) => {
  const cut = cutToGator(scene);
  const strip = cut - 58;
  const lapse = timelapse(TAKES, 8, cut - 6);
  const moves: Move[] = [
    { at: 4, box: BOXES.preview, zmax: 2.2 },
    { at: strip, box: BOXES.strip, zmax: 1.9 },
  ];
  return (
    <>
      <Desk layers={lapse} moves={moves} total={cut} />
      <Sequence from={cut} durationInFrames={scene.frames - cut} layout="none">
        <ClipShot name="gator" total={scene.frames - cut} push={0.04} />
        {sfx('impact-soft', 0, 0.6)}
      </Sequence>
    </>
  );
};

// ---- the montage --------------------------------------------------------------------------------------------------------
export const Montage: SceneFC = ({ scene }) => (
  <>
    <ClipShot name="montage" total={scene.frames} push={0.03} />
    {sfx('whoosh-short', 0, 0.5)}
  </>
);

export const End: SceneFC = ({ scene }) => <EndCard scene={scene} />;
