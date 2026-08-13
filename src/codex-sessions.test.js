import { expect, test } from 'bun:test';
import { describeRollout } from './codex-sessions.js';
import { sessionLabel } from './sessions.js';

const line = (obj) => JSON.stringify(obj) + '\n';
const meta = (payload = {}) =>
  line({
    timestamp: '2026-08-12T02:19:42.694Z',
    type: 'session_meta',
    payload: {
      session_id: '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c',
      cwd: 'F:\\bro-cli',
      originator: 'codex-tui',
      source: 'cli',
      thread_source: 'user',
      git: { branch: 'main' },
      ...payload
    }
  });
const userLine = (text, role = 'user') =>
  line({ type: 'response_item', payload: { type: 'message', role, content: [{ type: 'input_text', text }] } });
const eventLine = (message) => line({ type: 'event_msg', payload: { type: 'user_message', message } });
const plain = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');

test('a rollout is described from its own header, so nothing has to be guessed', () => {
  const text = meta() + userLine('fix the OAuth refresh');
  expect(describeRollout(text)).toEqual({
    id: '019ff3c3-7b7b-7f31-a66b-9eab4b230c5c',
    title: 'fix the OAuth refresh',
    cwd: 'F:\\bro-cli',
    branch: 'main',
    interactive: true
  });
});

test('the row title is the first prompt the user typed, not the context codex injected', () => {
  const text =
    meta() +
    userLine('You are Codex, an agent based on GPT-5.', 'developer') +
    userLine('# AGENTS.md instructions for F:\\bro-cli\n<INSTRUCTIONS>\nUse bun, not npm.') +
    userLine('<environment_context>\n<cwd>F:\\bro-cli</cwd>\n</environment_context>') +
    userLine('add codex sessions to the picker') +
    eventLine('add codex sessions to the picker') +
    userLine('and then ship it');

  expect(describeRollout(text).title).toBe('add codex sessions to the picker');
});

test('a prompt logged only as an event still titles the session', () => {
  expect(describeRollout(meta() + eventLine('deploy the api')).title).toBe('deploy the api');
});

test('a session opened by a slash command is named after the command, not dropped', () => {
  const text = meta() + userLine('<command-name>/model</command-name>');
  expect(describeRollout(text).title).toBe('/model');
});

test('a typed prompt outranks the slash command that opened the session', () => {
  const text = meta() + userLine('<command-name>/clear</command-name>') + userLine('now fix the picker');
  expect(describeRollout(text).title).toBe('now fix the picker');
});

test('a pasted screenshot does not become the title, and a wordless one still lists', () => {
  const withWords = meta() + userLine('<image name=[Image #1] path=C:\\shot.png> why is this button cut off');
  expect(describeRollout(withWords).title).toBe('why is this button cut off');

  // Some codex versions close the tag, leaving the marker on both sides.
  const closed = meta() + userLine('<image name=[Image #1]>shot</image> why is this button cut off');
  expect(describeRollout(closed).title).toBe('shot why is this button cut off');

  const wordless = meta() + userLine('<image name=[Image #1] path=C:\\shot.png>');
  expect(describeRollout(wordless).title).toBe('(image)');
});

test('a rollout holding nothing but injected context reports no title, so callers can drop it', () => {
  const text = meta() + userLine('# AGENTS.md instructions for F:\\bro-cli') + userLine('<codex_internal_context source="goal">continue</codex_internal_context>');
  expect(describeRollout(text).title).toBe('');
});

test('a rollout cut mid-line by the head read still yields what came before it', () => {
  const text = meta() + userLine('finish the picker') + '{"type":"response_item","payl';
  expect(describeRollout(text).title).toBe('finish the picker');
});

test('sub-agent and non-interactive rollouts are not offered for resume', () => {
  const subagent = meta({ thread_source: 'subagent', source: { subagent: { thread_spawn: { depth: 1 } } } });
  expect(describeRollout(subagent + userLine('explore the repo')).interactive).toBe(false);

  const exec = meta({ originator: 'codex_exec', source: 'exec' });
  expect(describeRollout(exec + userLine('run the deploy')).interactive).toBe(false);

  const desktop = meta({ originator: 'Codex Desktop', source: 'vscode', thread_source: undefined });
  expect(describeRollout(desktop + userLine('fix the flyer')).interactive).toBe(true);
});

test('a codex row names the login that owns it, exactly like a claude row', () => {
  const row = plain(
    sessionLabel(
      { title: 'fix the OAuth refresh', cwd: 'J:\\justgains', branch: 'main', account: 'work', mtime: Date.now() - 3600 * 1000 },
      { showPath: true }
    )
  );
  expect(row).toContain('J:\\justgains · main · work');

  const machine = plain(sessionLabel({ title: 'hello', cwd: 'F:\\bro-cli', account: null, mtime: Date.now() }));
  expect(machine).toContain('local');
});
