import { expect, test } from 'bun:test';
import { accountLabel, usageSummary } from './pool.js';

test('profile usage preserves the limits users use to choose an account', () => {
  const usage = usageSummary({
    five_hour: { utilization: 17 },
    seven_day: { utilization: 28 },
    limits: [
      { kind: 'weekly_scoped', percent: 53, scope: { model: { display_name: 'Fable' } } }
    ]
  });

  expect(usage).toEqual({ session: 17, weekly: 28, fable: 53 });
  expect(accountLabel({ name: 'James', authenticated: true, subscriptionType: 'max', usageStats: usage }))
    .toContain('5h 17% · week 28% · Fable 53%');
});

test('a stats outage does not hide or disable an authenticated profile', () => {
  const label = accountLabel({ name: 'work', authenticated: true, subscriptionType: 'team', usageStats: null });
  expect(label).toContain('work');
  expect(label).toContain('usage unavailable');
  expect(label).toContain('team');
});
