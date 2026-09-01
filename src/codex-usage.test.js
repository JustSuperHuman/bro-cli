import { expect, test } from 'bun:test';
import { EventEmitter } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { codexUsageSummary, fetchCodexUsage } from './codex-usage.js';

test('Codex backend usage is normalized into primary and secondary meters', () => {
  const usage = codexUsageSummary({
    plan_type: 'pro',
    rate_limit: {
      primary_window: {
        used_percent: 42,
        limit_window_seconds: 18_000,
        reset_at: 1_800_000_000
      },
      secondary_window: {
        used_percent: 7.5,
        limit_window_seconds: 604_800,
        reset_after_seconds: 120
      }
    },
    credits: { balance: 12.5, unlimited: false }
  });

  expect(usage.primary).toEqual({
    usedPercent: 42,
    windowDurationMins: 300,
    resetsAt: 1_800_000_000
  });
  expect(usage.secondary.usedPercent).toBe(7.5);
  expect(usage.secondary.windowDurationMins).toBe(10_080);
  expect(usage.secondary.resetsAt).toBeGreaterThan(Math.floor(Date.now() / 1000));
  expect(usage.planType).toBe('pro');
  expect(usage.credits).toEqual({ balance: 12.5, unlimited: false });
});

test('Codex app-server camelCase rate limits are accepted too', () => {
  const usage = codexUsageSummary({
    planType: 'team',
    primary: { usedPercent: 15, windowDurationMins: 60, resetsAt: 1234 }
  });
  expect(usage.primary).toEqual({ usedPercent: 15, windowDurationMins: 60, resetsAt: 1234 });
  expect(usage.secondary).toBeNull();
  expect(usage.planType).toBe('team');
});

test('the named Codex app-server limit wins over compatibility fallback limits', () => {
  const usage = codexUsageSummary({
    rateLimits: {
      primary: { usedPercent: 10, windowDurationMins: 300, resetsAt: 111 }
    },
    rateLimitsByLimitId: {
      codex: {
        primary: { usedPercent: 90, windowDurationMins: 10_080, resetsAt: 222 },
        secondary: null,
        planType: 'prolite',
        credits: { balance: '0', unlimited: false }
      }
    }
  });

  expect(usage.primary).toEqual({ usedPercent: 90, windowDurationMins: 10_080, resetsAt: 222 });
  expect(usage.planType).toBe('prolite');
  expect(usage.credits).toEqual({ balance: 0, unlimited: false });
});

test('Codex usage initializes the official app-server and reads its rate-limit method', async () => {
  const messages = [];
  let launched;
  const spawnProcess = (file, args, options) => {
    launched = { file, args, options };
    const child = new EventEmitter();
    child.exitCode = null;
    child.stdout = new PassThrough();
    child.stderr = new PassThrough();
    child.stdin = new Writable({
      write(chunk, _encoding, callback) {
        for (const line of String(chunk).trim().split('\n')) {
          if (!line) continue;
          const message = JSON.parse(line);
          messages.push(message);
          if (message.method === 'initialize') {
            queueMicrotask(() => child.stdout.write(`${JSON.stringify({ id: message.id, result: {} })}\n`));
          } else if (message.method === 'account/rateLimits/read') {
            queueMicrotask(() => child.stdout.write(`${JSON.stringify({
              id: message.id,
              result: {
                rateLimits: {
                  primary: { usedPercent: 33, windowDurationMins: 300, resetsAt: 123 }
                }
              }
            })}\n`));
          }
        }
        callback();
      }
    });
    child.kill = () => {
      child.exitCode = 0;
      queueMicrotask(() => child.emit('exit', 0));
      return true;
    };
    return child;
  };

  const usage = await fetchCodexUsage({
    home: 'C:\\profiles\\codex-work',
    executable: 'codex-test',
    spawnProcess,
    timeoutMs: 2000
  });

  expect(launched.file).toBe('codex-test');
  expect(launched.args).toEqual(['app-server', '--stdio']);
  expect(launched.options.env.CODEX_HOME).toBe('C:\\profiles\\codex-work');
  expect(messages.map((message) => message.method)).toEqual([
    'initialize', 'initialized', 'account/rateLimits/read'
  ]);
  expect(usage.primary).toEqual({ usedPercent: 33, windowDurationMins: 300, resetsAt: 123 });
});
