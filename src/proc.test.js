import { expect, test } from 'bun:test';
import {
  HARNESS_INSTALLS,
  ensureHarnessTool,
  globalInstallArgs,
  supportsDshNode,
  updateHarnessTool,
  windowsCmdLine
} from './proc.js';

test('every selectable harness has a cross-platform global package installer', () => {
  expect(Object.keys(HARNESS_INSTALLS)).toEqual(['claude', 'omp', 'pi', 'codex', 'dsh']);
  expect(HARNESS_INSTALLS.claude.packageName).toBe('@anthropic-ai/claude-code');
  expect(HARNESS_INSTALLS.omp.packageName).toBe('@oh-my-pi/pi-coding-agent');
  expect(HARNESS_INSTALLS.pi.packageName).toBe('@earendil-works/pi-coding-agent');
  expect(HARNESS_INSTALLS.codex.packageName).toBe('@openai/codex');
  expect(HARNESS_INSTALLS.dsh.packageName).toBe('@deepseek-ai/dsh');
  expect(globalInstallArgs(HARNESS_INSTALLS.dsh)).toEqual([
    'install', '-g', '@deepseek-ai/dsh@latest'
  ]);
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

test('an explicit harness update reinstalls latest even when the command already exists', () => {
  const calls = [];
  const result = updateHarnessTool('dsh', {
    binDirs: () => ['/global/bin'],
    find: (command) => command === 'npm' ? '/tools/npm' : command === 'dsh' ? '/global/bin/dsh' : null,
    run: (file, args) => {
      calls.push({ file, args });
      return { status: 0 };
    },
    announce: () => {}
  });

  expect(result.executable).toBe('/global/bin/dsh');
  expect(calls).toEqual([{
    file: '/tools/npm',
    args: ['install', '-g', '@deepseek-ai/dsh@latest']
  }]);
});

test('DeepSeek Harness enforces its upstream Node runtime floor', () => {
  expect(supportsDshNode('22.18.0')).toBe(false);
  expect(supportsDshNode('22.19.0')).toBe(true);
  expect(supportsDshNode('23.11.0')).toBe(false);
  expect(supportsDshNode('24.0.0')).toBe(true);
  expect(supportsDshNode('26.1.0')).toBe(true);
});

test('Windows command shims keep executable paths with spaces inside cmd outer quotes', () => {
  expect(windowsCmdLine('C:\\Program Files\\nodejs\\npm.cmd', [
    'install', '-g', '@deepseek-ai/dsh@latest'
  ])).toBe('""C:\\Program Files\\nodejs\\npm.cmd" install -g @deepseek-ai/dsh@latest"');
});
