const AGENTS = new Set(['claude', 'codex']);
const STATES = new Set(['active', 'inactive']);

// A private OSC envelope consumed by terminal-web's terminal metadata scanner.
// The JSON is base64url encoded so no payload byte can terminate or alter the
// control sequence. Keep this deliberately tiny: it classifies the foreground
// TUI, but never carries arguments, profiles, paths, session ids, or secrets.
export function terminalAgentOsc(agent, state) {
  if (!AGENTS.has(agent)) throw new TypeError(`Unsupported terminal agent: ${agent}`);
  if (!STATES.has(state)) throw new TypeError(`Unsupported terminal agent state: ${state}`);

  const payload = Buffer.from(JSON.stringify({ v: 1, agent, state }), 'utf8').toString('base64url');
  return `\x1b]1337;TerminalWeb.Agent=${payload}\x1b\\`;
}

// OSC belongs on an interactive terminal only. In particular, bro --print and
// other piped uses promise clean stdout and must never receive protocol bytes.
export function signalTerminalAgent(agent, state, { stream = process.stdout } = {}) {
  if (!agent || !stream?.isTTY || typeof stream.write !== 'function') return false;
  stream.write(terminalAgentOsc(agent, state));
  return true;
}

// Bracket the complete foreground lifetime, including failures and Ctrl+C
// exits reported by the child, so a reusable shell tab does not remain marked
// as an agent after control returns to its prompt.
export async function withTerminalAgent(agent, work, options) {
  const signalled = signalTerminalAgent(agent, 'active', options);
  try {
    return await work();
  } finally {
    if (signalled) signalTerminalAgent(agent, 'inactive', options);
  }
}
