// The questions (hooks/questions): the chime, the timer, its ticks. Run with
// `claude plugin test crates/bro-core/claude-mod`.
import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

const BIN = '/opt/bro/bro'

const QUESTIONS = [
  {
    question: 'Which colour?',
    header: 'Colour',
    multiSelect: false,
    options: [
      { label: 'Red', description: 'Warm' },
      { label: 'Blue', description: 'Cool' },
    ],
  },
]

const ANSWERED = { result: { questions: QUESTIONS, answers: { 'Which colour?': 'Blue' } } }

/** A bro session's world: a clock, bro's env, and the sounds bro was asked to play. */
function world(on: On) {
  const clock = mock.clock(on)
  mock.env(on, { BRO_SESSION_ID: 'sid-1', BRO_BIN: BIN })
  const chimes: number[] = []
  const players: { at: number; argv: readonly string[]; ended?: number }[] = []
  on('process.run', (_, e) => {
    if (e.argv.join(' ') === `${BIN} hook sound question`) {
      chimes.push(clock.now())
    }
    return { value: { exitCode: 0, stdout: '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  // A player that plays until it's stopped: ending the mod's stream aborts its
  // dispatch, which is what kills a real child.
  on('process.spawn', async function* (_, e, next) {
    const player: (typeof players)[number] = { at: clock.now(), argv: e.argv }
    players.push(player)
    next.signal.addEventListener('abort', () => {
      player.ended = clock.now()
    })
    await clock.sleep(600_000)
    return { value: { code: 0, signal: null } }
  })
  return { clock, chimes, players }
}

/** The engine's dialog: answered after `ms`, or never. */
function dialog(on: On, clock: ReturnType<typeof mock.clock>, ms = 600_000) {
  on('tool.call', async () => {
    await clock.sleep(ms)
    return ANSWERED
  })
}

test('a question chimes as it opens; without the timer it waits as long as it takes', async ($, on) => {
  const { clock, chimes, players } = world(on)
  dialog(on, clock, 120_000)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  expect(chimes).toEqual([0])
  await clock.advance(120_000)

  expect(await asked).toMatchObject(ANSWERED)
  expect(players).toHaveLength(0)
})

test('with the timer, an answer in time goes through and stops the ticks', { options: { timer: 30 } }, async ($, on) => {
  const { clock, players } = world(on)
  dialog(on, clock, 20_000)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(12_999)
  expect(players).toHaveLength(0)
  // the player starts two seconds before the first tick, and times the ticks itself
  await clock.advance(1)
  expect(players).toHaveLength(1)
  expect(players[0]?.argv).toEqual([BIN, 'hook', 'sound', 'ticks', '30000', '15'])

  await clock.advance(7_000)
  expect(await asked).toMatchObject(ANSWERED)
  expect(players[0]?.ended).toBe(20_000)
})

test('with no answer by the deadline, Claude goes with its best judgement', { options: { timer: 30 } }, async ($, on) => {
  const { clock } = world(on)
  dialog(on, clock)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(30_000)

  const result = await asked
  expect(result).toMatchObject({ result: { answers: {}, afkTimeoutMs: 30_000 } })
  expect(result.context?.[0]).toMatch(/within 30 seconds.*best judgement/)
})

test('a timer shorter than the ticks ticks from the start', { options: { timer: 10 } }, async ($, on) => {
  const { clock, players } = world(on)
  dialog(on, clock)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  expect(players[0]?.argv).toEqual([BIN, 'hook', 'sound', 'ticks', '10000', '10'])
  await clock.advance(10_000)
  expect(await asked).toMatchObject({ result: { afkTimeoutMs: 10_000 } })
})

test('the chime and the ticks are optional', { options: { timer: 30, chime: false, ticks: false } }, async ($, on) => {
  const { clock, chimes, players } = world(on)
  dialog(on, clock)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(30_000)

  expect(await asked).toMatchObject({ result: { afkTimeoutMs: 30_000 } })
  expect(chimes).toEqual([])
  expect(players).toEqual([])
})
