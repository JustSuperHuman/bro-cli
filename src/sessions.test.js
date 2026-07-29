import { expect, test } from 'bun:test';
import { describeTranscript, encodeProjectDir, relativeTime, sessionLabel, shortPath } from './sessions.js';

const line = (obj) => JSON.stringify(obj) + '\n';
const userLine = (content, extra = {}) => line({ type: 'user', message: { role: 'user', content }, ...extra });
const plain = (s) => s.replace(/\x1b\[[0-9;]*m/g, '');

test('a project directory encodes the way Claude Code encodes it', () => {
  expect(encodeProjectDir('F:\\bro-cli')).toBe('F--bro-cli');
  expect(encodeProjectDir('C:\\Users\\james\\.ssh')).toBe('C--Users-james--ssh');
});

test('the row title is the first prompt the user actually typed', () => {
  const text =
    line({ type: 'mode', mode: 'normal' }) +
    userLine('<local-command-caveat>Caveat: The messages below were generated…</local-command-caveat>') +
    userLine('<command-name>/clear</command-name>') +
    userLine('fix the OAuth refresh', { cwd: 'F:\\bro-cli', gitBranch: 'main' }) +
    userLine('and then deploy it');

  expect(describeTranscript(text)).toEqual({ title: 'fix the OAuth refresh', cwd: 'F:\\bro-cli', branch: 'main' });
});

test('a compacted session is titled by its own summary, not by a later prompt', () => {
  const text =
    line({ type: 'summary', summary: 'Pool proxy token refresh' }) +
    userLine('continue where we left off', { cwd: 'J:\\justgains' });

  expect(describeTranscript(text).title).toBe('Pool proxy token refresh');
});

test('a pasted image does not become the title, and block content still reads', () => {
  const text = userLine([
    { type: 'text', text: '[Image: original 1080x2400, displayed at 900x2000.] why is this button cut off' }
  ]);

  expect(describeTranscript(text).title).toBe('why is this button cut off');
});

test('a session opened by a slash command is named after the command, not dropped', () => {
  const text =
    line({ type: 'mode', mode: 'normal' }) +
    userLine('<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>PR 42</command-args>') +
    userLine([{ type: 'text', text: 'Base directory for this skill: C:\\skills\\review' }], { isMeta: true });

  expect(describeTranscript(text).title).toBe('/review PR 42');
});

test('a typed prompt outranks the slash command that opened the session', () => {
  const text = userLine('<command-name>/clear</command-name>') + userLine('now fix the picker');
  expect(describeTranscript(text).title).toBe('now fix the picker');
});

test('a session with no user entry at all reports no title, so callers can drop it', () => {
  const text =
    line({ type: 'mode', mode: 'normal' }) +
    userLine('a meta note', { isMeta: true }) +
    userLine('a subagent prompt', { isSidechain: true });

  expect(describeTranscript(text).title).toBe('');
});

test('a transcript cut mid-line by the head read still yields what came before it', () => {
  const text = userLine('finish the picker', { cwd: 'F:\\bro-cli' }) + '{"type":"user","messa';
  expect(describeTranscript(text).title).toBe('finish the picker');
});

test('a prompt that opens with a pasted image survives the truncated first line', () => {
  // Base64 image data makes this entry megabytes long, so the head read always
  // cuts it mid-object — there is no complete JSON line to parse.
  const cut = line({
    type: 'user',
    message: { role: 'user', content: [{ type: 'image', source: { data: 'A'.repeat(4000) } }, { type: 'text', text: 'why is this button cut off' }] }
  }).slice(0, 3000);
  expect(describeTranscript(cut).title).toBe('(image)');

  const withText = line({
    type: 'user',
    message: { role: 'user', content: [{ type: 'text', text: 'why is this button cut off' }, { type: 'image', source: { data: 'A'.repeat(4000) } }] }
  }).slice(0, 3000);
  expect(describeTranscript(withText).title).toBe('why is this button cut off');
});

test('session ages read as glanceable durations', () => {
  const now = Date.UTC(2026, 6, 29, 12, 0, 0);
  const ago = (ms) => relativeTime(now - ms, now);
  expect(ago(5 * 1000)).toBe('now');
  expect(ago(20 * 60 * 1000)).toBe('20m');
  expect(ago(5 * 3600 * 1000)).toBe('5h');
  expect(ago(3 * 86400 * 1000)).toBe('3d');
  expect(ago(21 * 86400 * 1000)).toBe('3w');
});

test('a long path keeps the tail, which is the part that names the project', () => {
  expect(shortPath('J:\\justgains', 34)).toBe('J:\\justgains');
  const long = shortPath('J:\\very\\deeply\\nested\\workspace\\project-name', 20);
  expect(long).toBe('…kspace\\project-name');
  expect(long.length).toBe(20);
});

test('a session row shows age, prompt, and the profile that can resume it', () => {
  const row = plain(
    sessionLabel(
      { title: 'fix the OAuth refresh', cwd: 'J:\\justgains', branch: 'main', account: 'claude-2', mtime: Date.now() - 3600 * 1000 },
      { showPath: true }
    )
  );
  expect(row).toContain('1h');
  expect(row).toContain('fix the OAuth refresh');
  expect(row).toContain('J:\\justgains · main · claude-2');
});

test('a session from the machine\'s own login is labelled local, not left untagged', () => {
  const row = plain(sessionLabel({ title: 'hello', cwd: 'F:\\bro-cli', branch: '', account: null, mtime: Date.now() }));
  expect(row).toContain('local');
  expect(row).not.toContain('F:\\bro-cli'); // this project's rows don't repeat the path
});
