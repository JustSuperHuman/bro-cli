import readline from 'node:readline';

const stdin = process.stdin;
const stdout = process.stdout;

export const isInteractive = Boolean(stdin.isTTY && stdout.isTTY);

// A tiny zero-dependency arrow-key list selector. Works in Windows Terminal,
// conhost, macOS and Linux (Node's readline normalises the key sequences).
// Returns the chosen item, or throws Error('cancelled') on Esc / Ctrl-C.
// Optional `toggle` adds an on/off switch (flipped with Tab/Space) shown under the
// list — handy for things like "Skip permissions". When present, the resolved
// object carries `toggleOn` with the final state.
export function select({ message, choices, startIndex = 0, toggle = null }) {
  if (!isInteractive) {
    return Promise.reject(new Error('A terminal (TTY) is required to choose interactively. Use --provider / --model instead.'));
  }
  return new Promise((resolve, reject) => {
    let index = Math.max(0, Math.min(startIndex, choices.length - 1));
    let on = toggle ? Boolean(toggle.value) : false;
    const lines = choices.length + 1 + (toggle ? 1 : 0); // message + choices (+ toggle row)
    const hint = `\x1b[2m  ↑/↓ move · enter select${toggle ? ' · tab toggle' : ''} · esc cancel\x1b[0m`;

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();

    const toggleRow = () => {
      const state = on ? '\x1b[30;42m ON  \x1b[0m' : '\x1b[97;41m OFF \x1b[0m';
      return `  \x1b[2m[tab]\x1b[0m ${toggle.label}  ${state}`;
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
        resolve(toggle ? { ...choices[index], toggleOn: on } : choices[index]);
      } else if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        cleanup();
        reject(new Error('cancelled'));
      }
    };

    stdin.on('keypress', onKey);
    paint(true);
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
