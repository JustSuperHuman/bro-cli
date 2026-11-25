// The questions (hooks/questions): the chime, the timer, its ticks. Run with
// `claude plugin test crates/bro-core/claude-mod`.
import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'
import type { TestBody } from 'claude-code/testing'

const BIN = '/opt/bro/bro'
const BRIDGE = 'http://127.0.0.1:10001'
const TYPED = `${BRIDGE}/api/mod/sid-1/typed`

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
function world(on: On, env: Record<string, string> = {}) {
  const clock = mock.clock(on)
  mock.env(on, { BRO_SESSION_ID: 'sid-1', BRO_BIN: BIN, ...env })
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

/** The engine's dialog: answered after `ms`, or never. Answers the ids of the calls it drew. */
function dialog(on: On, clock: ReturnType<typeof mock.clock>, ms = 600_000) {
  const ids: string[] = []
  on('tool.call', async (_, e) => {
    ids.push(e.tool_use_id)
    await clock.sleep(ms)
    return ANSWERED
  })
  return ids
}

/**
 * bro's bridge, saying how long ago the person last typed into the session
 * (`typedAt`, the clock's time, or null before they have). `canSay` false is a
 * bro from before it could.
 */
function bridge(on: On, clock: ReturnType<typeof mock.clock>) {
  const bro = { typedAt: null as number | null, asked: 0, canSay: true }
  on('http.fetch', (_, e) => {
    if (e.url !== TYPED) {
      return { deny: 'connection refused' }
    }
    bro.asked += 1
    const idleMs = bro.typedAt === null ? null : clock.now() - bro.typedAt
    const [status, body] = bro.canSay ? [200, { idleMs }] : [404, { message: 'not here' }]
    return { value: { status, ok: status < 300, headers: {}, text: JSON.stringify(body) } }
  })
  return bro
}

/** The countdown's line above the dialog as the terminal draws it, by the call's id. */
function countdown($: Parameters<TestBody>[0], on: On) {
  // the engine's own dialog beneath the mod: core's drawing of the original props
  on('ui.render', () => ({ type: 'engine', ref: 0 }))
  return async (id: string | undefined) => {
    const ui = await $.ui.mount({
      plugin: 'bro',
      surface: 'terminal',
      component: 'AskUserQuestion',
      requestId: id,
      props: { tool: 'AskUserQuestion', questions: QUESTIONS },
    })
    const line = await ui.find({ type: 'Text', text: /\d+s / })
    await ui.unmount()
    return line?.text
  }
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

test('starting to answer stops the countdown; left a minute, it runs on from where it stopped', { options: { timer: 30 } }, async ($, on) => {
  const { clock, players } = world(on, { BRO_BRIDGE_URL: BRIDGE })
  const bro = bridge(on, clock)
  const ids = dialog(on, clock)
  const line = countdown($, on)
  let isClosed = false
  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS }).then(result => {
    isClosed = true
    return result
  })
  await clock.settle()
  await clock.advance(10_000)
  expect(await line(ids[0])).toMatch(/20s then Claude goes with its best judgement/)

  // a key at 10s: the next look hears it, and the countdown stops at 19s
  bro.typedAt = 10_000
  await clock.advance(1_000)
  expect(await line(ids[0])).toMatch(/19s paused while you answer/)
  await clock.advance(19_000)
  expect(isClosed).toBe(false)
  expect(players).toHaveLength(0)

  // a minute after the key it runs again, 19 seconds to go, the ticks timed to the new close
  await clock.advance(39_000)
  expect(await line(ids[0])).toMatch(/19s paused/)
  await clock.advance(1_000)
  expect(await line(ids[0])).toMatch(/19s then Claude/)
  await clock.advance(2_000)
  expect(players[0]?.argv).toEqual([BIN, 'hook', 'sound', 'ticks', '89000', '15'])

  await clock.advance(17_000)
  const result = await asked
  expect(result).toMatchObject({ result: { answers: {}, afkTimeoutMs: 79_000 } })
  expect(result.context?.[0]).toMatch(/started on the question.*over a minute.*best judgement/)
  expect(players[0]?.ended).toBe(89_000)
})

test("a key in the countdown's last second still stops it, and the answer goes through", { options: { timer: 30 } }, async ($, on) => {
  const { clock, players } = world(on, { BRO_BRIDGE_URL: BRIDGE })
  const bro = bridge(on, clock)
  dialog(on, clock, 45_000)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(29_500)
  expect(players).toHaveLength(1)
  bro.typedAt = 29_500
  await clock.advance(500)
  // stopped at zero: the ticks with it
  expect(players[0]?.ended).toBe(30_000)

  await clock.advance(15_000)
  expect(await asked).toMatchObject(ANSWERED)
})

test('keys from before the question opened leave its countdown running', { options: { timer: 30 } }, async ($, on) => {
  const { clock } = world(on, { BRO_BRIDGE_URL: BRIDGE })
  const bro = bridge(on, clock)
  bro.typedAt = -4_000
  dialog(on, clock)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(30_000)
  expect(await asked).toMatchObject({ result: { answers: {}, afkTimeoutMs: 30_000 } })
})

test("a bro that can't say when you typed is asked once, and the countdown runs as before", { options: { timer: 30 } }, async ($, on) => {
  const { clock } = world(on, { BRO_BRIDGE_URL: BRIDGE })
  const bro = bridge(on, clock)
  bro.canSay = false
  dialog(on, clock)

  const asked = $.tool.call({ tool: 'AskUserQuestion', questions: QUESTIONS })
  await clock.settle()
  await clock.advance(30_000)
  const result = await asked
  expect(result).toMatchObject({ result: { answers: {}, afkTimeoutMs: 30_000 } })
  expect(result.context?.[0]).toMatch(/within 30 seconds/)
  expect(bro.asked).toBe(1)
})
