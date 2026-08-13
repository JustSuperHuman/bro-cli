import { expect, test } from 'bun:test';
import { HARNESS_INSTALLS, ensureHarnessTool, globalInstallArgs } from './proc.js';

test('every selectable harness has a cross-platform global package installer', () => {
  expect(Object.keys(HARNESS_INSTALLS)).toEqual(['claude', 'omp', 'pi', 'codex']);
  expect(HARNESS_INSTALLS.claude.packageName).toBe('@anthropic-ai/claude-code');
  expect(HARNESS_INSTALLS.omp.packageName).toBe('@oh-my-pi/pi-coding-agent');
  expect(HARNESS_INSTALLS.pi.packageName).toBe('@earendil-works/pi-coding-agent');
  expect(HARNESS_INSTALLS.codex.packageName).toBe('@openai/codex');
  expect(globalInstallArgs(HARNESS_INSTALLS.pi)).toEqual([
    'install', '-g', '--ignore-scripts', '@earendil-works/pi-coding-agent'
  ]);
});

test('a missing harness installs once and is resolved from refreshed global bins', () => {
  for (const [name, spec] of Object.entries(HARNESS_INSTALLS)) {
    let installed = false;
    const calls = [];
    const manager = spec.managers[0];
    const result = ensureHarnessTool(name, {
      binDirs: () => ['/global/bin'],
      find: (command) => {
        if (command === spec.command) return installed ? `/global/bin/${spec.command}` : null;
        if (command === manager) return `/tools/${manager}`;
        return null;
      },
      run: (file, args) => {
        calls.push({ file, args });
        installed = true;
        return { status: 0 };
      },
      announce: () => {}
    });

    expect(result.executable).toBe(`/global/bin/${spec.command}`);
    expect(calls).toEqual([{ file: `/tools/${manager}`, args: globalInstallArgs(spec) }]);
  }
});

test('an installed harness never invokes a package manager', () => {
  const result = ensureHarnessTool('pi', {
    binDirs: () => ['/already'],
    find: (command) => command === 'pi' ? '/already/pi' : null,
    run: () => { throw new Error('installer should not run'); },
    announce: () => {}
  });
  expect(result.executable).toBe('/already/pi');
});
