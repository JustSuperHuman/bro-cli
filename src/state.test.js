import { afterAll, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { lastHarness, lastModelFor, lastProvider, rememberHarness, rememberSelection } from './state.js';

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-state-'));
const previousStatePath = process.env.BRO_STATE_PATH;
process.env.BRO_STATE_PATH = path.join(root, 'state.json');

afterAll(() => {
  if (previousStatePath == null) delete process.env.BRO_STATE_PATH;
  else process.env.BRO_STATE_PATH = previousStatePath;
  if (path.dirname(path.resolve(root)) !== path.resolve(os.tmpdir())) throw new Error('Unexpected temporary test path');
  fs.rmSync(root, { recursive: true, force: true });
});

test('the last harness is saved independently and preserves provider/model state', () => {
  rememberSelection('openrouter', 'openai/gpt-5', 'claude');
  rememberHarness('pi');

  expect(lastHarness()).toBe('pi');
  expect(lastProvider()).toBe('openrouter');
  expect(lastModelFor('openrouter')).toBe('openai/gpt-5');
});
