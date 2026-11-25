import React from 'react';
import { Composition } from 'remotion';
import { Film } from './Film';
import { FPS, H, W } from './theme';
import { TOTAL } from './timeline';

export const Root: React.FC = () => <Composition id="Film" component={Film} durationInFrames={TOTAL} fps={FPS} width={W} height={H} />;
