import React from 'react';
import { Composition } from 'remotion';
import { Story } from './Story';
import { H, W } from './theme';
import { FPS, TOTAL } from './timeline';

export const Root: React.FC = () => <Composition id="Story" component={Story} durationInFrames={TOTAL} fps={FPS} width={W} height={H} />;
