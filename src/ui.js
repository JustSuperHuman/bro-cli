import readline from 'node:readline';

const stdin = process.stdin;
const stdout = process.stdout;

export const isInteractive = Boolean(stdin.isTTY && stdout.isTTY);

// A tiny zero-dependency arrow-key list selector. Works in Windows Terminal,
// conhost, macOS and Linux (Node's readline normalises the key sequences).
// Returns the chosen item, or throws Error('cancelled') on Esc / Ctrl-C.
// Optional `toggle` adds an on/off switch (flipped with Tab/Space) shown under the
// list — handy for things like "Skip permissions". `toggles` can add extra
// keyed switches, each returned by name in `toggles`.
export function select({ message, choices, startIndex = 0, toggle = null, toggles = [] }) {
  if (!isInteractive) {
    return Promise.reject(new Error('A terminal (TTY) is required to choose interactively. Use --provider / --model instead.'));
  }
  return new Promise((resolve, reject) => {
    let index = Math.max(0, Math.min(startIndex, choices.length - 1));
    let on = toggle ? Boolean(toggle.value) : false;
    const keyed = toggles.map((t) => ({ ...t, value: Boolean(t.value) }));
    const lines = choices.length + 1 + (toggle ? 1 : 0) + keyed.length; // message + choices (+ toggle rows)
    const extraHints = [
      toggle ? 'tab toggle' : null,
      ...keyed.map((t) => `${t.key} ${t.shortLabel || 'toggle'}`)
    ].filter(Boolean);
    const hint = `\x1b[2m  ↑/↓ move · enter select${extraHints.length ? ' · ' + extraHints.join(' · ') : ''} · esc cancel\x1b[0m`;

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();

    const toggleRow = () => {
      const state = on ? '\x1b[30;42m ON  \x1b[0m' : '\x1b[97;41m OFF \x1b[0m';
      return `  \x1b[2m[tab]\x1b[0m ${toggle.label}  ${state}`;
    };

    // A keyed toggle with both labels is a choice between two options, not an
    // on/off state — show both side by side with the active one highlighted.
    // Only single-label toggles get the green/red ON/OFF treatment.
    const keyedRow = (t) => {
      let state;
      if (t.onLabel && t.offLabel) {
        const seg = (label, active) => (active ? `\x1b[7m ${label} \x1b[0m` : `\x1b[2m ${label} \x1b[0m`);
        state = seg(t.offLabel, !t.value) + '\x1b[2m│\x1b[0m' + seg(t.onLabel, t.value);
      } else {
        state = t.value ? `\x1b[30;42m ${t.onLabel || 'ON'} \x1b[0m` : `\x1b[97;41m ${t.offLabel || 'OFF'} \x1b[0m`;
      }
      return `  \x1b[2m[${t.key}]\x1b[0m ${t.label}  ${state}`;
    };

    const paint = (first) => {
      if (!first) {
        readline.cursorTo(stdout, 0);
        readline.moveCursor(stdout, 0, -lines);
      }
      readline.clearScreenDown(stdout);
      stdout.write(`\x1b[1m${message}\x1b[0m\n`);
      choices.forEach((c, i) => {
        const label = c.label ?? c.name ?? String(c.value);
        stdout.write(i === index ? `\x1b[7m ❯ ${label} \x1b[0m\n` : `   ${label}\n`);
      });
      if (toggle) stdout.write(toggleRow() + '\n');
      for (const t of keyed) stdout.write(keyedRow(t) + '\n');
      stdout.write(hint);
    };

    const cleanup = () => {
      stdin.removeListener('keypress', onKey);
      stdin.setRawMode(false);
      stdin.pause();
      stdout.write('\n');
    };

    const onKey = (str, key) => {
      if (!key) return;
      if (key.name === 'up' || key.name === 'k') {
        index = (index - 1 + choices.length) % choices.length;
        paint(false);
      } else if (key.name === 'down' || key.name === 'j') {
        index = (index + 1) % choices.length;
        paint(false);
      } else if (toggle && (key.name === 'tab' || key.name === 'space')) {
        on = !on;
        paint(false);
      } else if (key.name === 'return' || key.name === 'enter') {
        cleanup();
        const choice = { ...choices[index] };
        if (toggle) choice.toggleOn = on;
        if (keyed.length) choice.toggles = Object.fromEntries(keyed.map((t) => [t.name || t.key, t.value]));
        resolve(choice);
      } else if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        cleanup();
        reject(new Error('cancelled'));
      } else {
        const t = keyed.find((item) => key.name === item.key || str === item.key);
        if (t) {
          t.value = !t.value;
          paint(false);
        }
      }
    };

    stdin.on('keypress', onKey);
    paint(true);
  });
}

// Show something for `ms`, then continue — but let the user steer:
//   enter            -> continue immediately (skip the remaining wait)
//   any other key    -> pause: stop the timer and wait for enter
//   ctrl-c / esc     -> cancel (resolves false)
// Resolves true to continue, false to cancel. On a non-TTY it just continues.
export function holdOrContinue({ ms = 1500, pausedMessage = 'Paused — press enter to launch, esc to cancel.' } = {}) {
  if (!isInteractive) return Promise.resolve(true);
  return new Promise((resolve) => {
    let timer = null;
    let paused = false;
    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();

    const finish = (val) => {
      if (timer) clearTimeout(timer);
      stdin.removeListener('keypress', onKey);
      stdin.setRawMode(false);
      stdin.pause();
      resolve(val);
    };

    const onKey = (_str, key) => {
      if (!key) return;
      if (key.name === 'return' || key.name === 'enter') return finish(true);
      if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        stdout.write('\n');
        return finish(false);
      }
      if (!paused) {
        paused = true;
        if (timer) { clearTimeout(timer); timer = null; }
        stdout.write(`\n\x1b[2m${pausedMessage}\x1b[0m`);
      }
    };

    stdin.on('keypress', onKey);
    timer = setTimeout(() => finish(true), ms);
  });
}

// Prompt for a visible line of input. Resolves with the trimmed string (may be
// empty if the user just hits enter). Rejects on Ctrl-C.
export function prompt(message) {
  if (!isInteractive) return Promise.reject(new Error('A terminal (TTY) is required for input.'));
  return new Promise((resolve, reject) => {
    const rl = readline.createInterface({ input: stdin, output: stdout });
    rl.question(message, (answer) => {
      rl.close();
      resolve((answer || '').trim());
    });
    rl.on('SIGINT', () => {
      rl.close();
      stdout.write('\n');
      reject(new Error('cancelled'));
    });
  });
}

// Prompt for a secret, echoing nothing.
export function promptHidden(message) {
  if (!isInteractive) return Promise.reject(new Error('A terminal (TTY) is required to enter a key.'));
  return new Promise((resolve) => {
    stdout.write(message);
    let value = '';
    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();
    const onKey = (str, key) => {
      if (key && (key.name === 'return' || key.name === 'enter')) {
        stdin.removeListener('keypress', onKey);
        stdin.setRawMode(false);
        stdin.pause();
        stdout.write('\n');
        resolve(value.trim());
      } else if (key && key.ctrl && key.name === 'c') {
        stdout.write('\n');
        process.exit(130);
      } else if (key && key.name === 'backspace') {
        value = value.slice(0, -1);
      } else if (str) {
        value += str;
      }
    };
    stdin.on('keypress', onKey);
  });
}
