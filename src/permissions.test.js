import { expect, test } from 'bun:test';
import { configPermissionMode } from './config.js';
import { launch, permissionArgs } from './launch.js';
import { runPool, runAccountProfile } from './pool.js';

test('permission config defaults to auto and preserves explicit and legacy choices', () => {
  expect(configPermissionMode()).toBe('auto');
  expect(configPermissionMode({ dangerouslySkipPermissions: true })).toBe('bypass');
  expect(configPermissionMode({ dangerouslySkipPermissions: false })).toBe('manual');
  expect(configPermissionMode({ permissionMode: 'auto', dangerouslySkipPermissions: true })).toBe('auto');
  expect(permissionArgs('auto')).toEqual(['--permission-mode', 'auto']);
  expect(permissionArgs('manual')).toEqual([]);
  expect(permissionArgs('bypass')).toEqual(['--dangerously-skip-permissions']);
});

test('Claude native, proxy, pool and account launches honor each permission mode', async () => {
  for (const permissionMode of ['auto', 'manual', 'bypass']) {
    const plans = [
      await launch({ provider: { id: 'anthropic', mode: 'native' }, permissionMode, dryRun: true }),
      await launch({ provider: { id: 'test', mode: 'openai' }, model: 'test', permissionMode, dryRun: true }),
      (await runPool({ permissionMode, dryRun: true })).claude,
      (await runAccountProfile({ accountName: 'test', permissionMode, dryRun: true })).claude
    ];
    for (const plan of plans) {
      expect(plan.args.includes('--dangerously-skip-permissions')).toBe(permissionMode === 'bypass');
      expect(plan.args.includes('--permission-mode')).toBe(permissionMode === 'auto');
    }
  }
});
