import { expect, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { isHeadlessRun, parseArgs } from './cli.js';
import { parseImagineArgs } from './justimagine.js';

const src = path.dirname(fileURLToPath(import.meta.url));

test('--print takes an optional prompt and never eats a following flag', () => {
  expect(parseArgs(['--print', 'hello there']).print).toBe('hello there');
  // No prompt: the harness reads it from stdin, so an empty string still
  // counts as "print mode was asked for".
  expect(parseArgs(['--print']).print).toBe('');
  const withFlags = parseArgs(['--print', '--output-format', 'json']);
  expect(withFlags.print).toBe('');
  expect(withFlags._).toEqual(['--output-format', 'json']);
});

// The bare form stops at the next flag, so the attached form is the only way a
// dash-leading prompt reaches bro intact. (main() then turns it away with the
// stdin recipe, because Claude Code's own parser reads it as a flag too.)
test('--print= keeps a prompt that begins with a dash', () => {
  expect(parseArgs(['--print=-v means verbose, right?']).print).toBe('-v means verbose, right?');
  expect(parseArgs(['--print=']).print).toBe('');
  const bare = parseArgs(['--print', '-v means verbose']);
  expect(bare.print).toBe('');
  expect(bare._).toEqual(['-v means verbose']);
});

test('--print composes with provider and model, and with pass-through args', () => {
  const args = parseArgs(['-p', 'zai', '-m', 'glm-5.3', '--print', 'hi', '--output-format', 'json']);
  expect(args).toMatchObject({ provider: 'zai', model: 'glm-5.3', print: 'hi' });
  expect(args._).toEqual(['--output-format', 'json']);
});

// bro's own -p is --provider; Claude Code's -p is print mode. After `--` the
// harness owns the flag, which is exactly how it was reachable before --print.
test('a -p after -- belongs to the harness, not to bro', () => {
  const args = parseArgs(['-p', 'zai', '--', '-p', 'prompt text']);
  expect(args.provider).toBe('zai');
  expect(args.print).toBeUndefined();
  expect(args._).toEqual(['-p', 'prompt text']);
});

// `bro image` was the name for years; it has to keep working, alongside the
// new one, without either becoming a provider/model selection.
test('every spelling of the JustImagine word routes to the gallery', () => {
  for (const word of ['imagine', 'justimagine', 'image', 'image-gen', '--image', '--imagine']) {
    expect(parseArgs([word]).imagine).toBe(true);
  }
  expect(parseArgs(['imagine', '-p', 'openrouter'])).toMatchObject({ imagine: true, provider: 'openrouter' });
  expect(parseArgs(['-p', 'zai']).imagine).toBeUndefined();
});

test('gallery flags are parsed apart from the harness flags', () => {
  const a = parseImagineArgs(['--root', 'D:/Art', '--port', '9000', '--no-open', '-p', 'yunwu']);
  expect(a).toMatchObject({ root: 'D:/Art', port: 9000, open: false, api: 'yunwu' });
  expect(a._).toEqual([]);
  // nothing set is nothing assumed — the defaults live in runJustImagine
  expect(parseImagineArgs([])).toEqual({ _: [] });
});

test('a run is headless when it prints, or when there is no terminal', () => {
  expect(isHeadlessRun({ print: 'hi', interactive: true })).toBe(true);
  expect(isHeadlessRun({ print: '', interactive: true })).toBe(true);
  expect(isHeadlessRun({ harnessArgs: ['-p', 'prompt'], interactive: true })).toBe(true);
  expect(isHeadlessRun({ harnessArgs: ['--print'], interactive: true })).toBe(true);
  expect(isHeadlessRun({ harnessArgs: [], interactive: false })).toBe(true);
  expect(isHeadlessRun({ harnessArgs: ['--resume', 'abc'], interactive: true })).toBe(false);
});

// The whole point of the exercise: stdout carries the model's answer and
// nothing else, so `bro … --print … --output-format json | jq` works.
test('bro talks about itself on stderr, leaving stdout to the harness', () => {
  const script = [
    `import { note } from ${JSON.stringify(path.join(src, 'out.js').replace(/\\/g, '/'))};`,
    'note("Launching Something / a-model…");',
    'process.stdout.write("HARNESS OUTPUT");'
  ].join('\n');
  const run = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8' });
  expect(run.stdout).toBe('HARNESS OUTPUT');
  expect(run.stderr).toContain('Launching Something');
});
