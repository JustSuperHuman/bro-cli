import { expect, test } from 'bun:test';
import { signalTerminalAgent, terminalAgentOsc, withTerminalAgent } from './terminal-agent.js';

function decode(sequence) {
  const match = /^\x1b\]1337;TerminalWeb\.Agent=([^\x1b]+)\x1b\\$/.exec(sequence);
  expect(match).not.toBeNull();
  return JSON.parse(Buffer.from(match[1], 'base64url').toString('utf8'));
}

test('terminal agent OSC contains only the fixed version, agent, and lifecycle state', () => {
  expect(decode(terminalAgentOsc('claude', 'active'))).toEqual({ v: 1, agent: 'claude', state: 'active' });
  expect(decode(terminalAgentOsc('codex', 'inactive'))).toEqual({ v: 1, agent: 'codex', state: 'inactive' });
  expect(() => terminalAgentOsc('dsh', 'active')).toThrow(/Unsupported terminal agent/);
  expect(() => terminalAgentOsc('claude', 'resume')).toThrow(/Unsupported terminal agent state/);
});

test('terminal agent metadata is silent unless stdout is a TTY', () => {
  const writes = [];
  const pipe = { isTTY: false, write: (value) => writes.push(value) };
  const tty = { isTTY: true, write: (value) => writes.push(value) };

  expect(signalTerminalAgent('claude', 'active', { stream: pipe })).toBe(false);
  expect(writes).toEqual([]);
  expect(signalTerminalAgent('claude', 'active', { stream: tty })).toBe(true);
  expect(writes).toHaveLength(1);
  expect(decode(writes[0])).toEqual({ v: 1, agent: 'claude', state: 'active' });
});

test('foreground metadata always clears, including when the launch fails', async () => {
  const writes = [];
  const stream = { isTTY: true, write: (value) => writes.push(decode(value)) };

  await expect(withTerminalAgent('codex', async () => {
    throw new Error('launch failed');
  }, { stream })).rejects.toThrow('launch failed');

  expect(writes).toEqual([
    { v: 1, agent: 'codex', state: 'active' },
    { v: 1, agent: 'codex', state: 'inactive' }
  ]);
});
