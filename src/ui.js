import readline from 'node:readline';

const stdin = process.stdin;
const stdout = process.stdout;

export const isInteractive = Boolean(stdin.isTTY && stdout.isTTY);

// ---------- ANSI-aware text measurement (for column layouts) ----------

const ANSI_RE = /\x1b\[[0-9;]*m/g;
const stripAnsi = (s) => String(s ?? '').replace(ANSI_RE, '');

// Approximate terminal cell width — CJK and emoji occupy two columns.
function charWidth(cp) {
  return (cp >= 0x1100 && cp <= 0x115f) ||
    (cp >= 0x2e80 && cp <= 0x303e) ||
    (cp >= 0x3041 && cp <= 0x33ff) ||
    (cp >= 0x3400 && cp <= 0x4dbf) ||
    (cp >= 0x4e00 && cp <= 0x9fff) ||
    (cp >= 0xa000 && cp <= 0xa4cf) ||
    (cp >= 0xac00 && cp <= 0xd7a3) ||
    (cp >= 0xf900 && cp <= 0xfaff) ||
    (cp >= 0xfe30 && cp <= 0xfe4f) ||
    (cp >= 0xff00 && cp <= 0xff60) ||
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1faff) ||
    cp >= 0x20000
    ? 2
    : 1;
}

function visWidth(s) {
  let w = 0;
  for (const ch of stripAnsi(s)) w += charWidth(ch.codePointAt(0));
  return w;
}

// Pad or truncate to exactly `width` visible columns. ANSI codes pass through
// (they're zero-width) and a reset is appended so styles can't leak into the
// padding or the next cell.
function fit(s, width) {
  s = String(s ?? '');
  const overflow = visWidth(s) > width;
  const budget = overflow ? Math.max(0, width - 1) : width;
  let out = '';
  let w = 0;
  let hadAnsi = false;
  let done = false;
  for (const part of s.split(/(\x1b\[[0-9;]*m)/)) {
    if (!part) continue;
    if (part.startsWith('\x1b[')) {
      out += part;
      hadAnsi = true;
      continue;
    }
    if (done) continue;
    for (const ch of part) {
      const cw = charWidth(ch.codePointAt(0));
      if (w + cw > budget) {
        done = true;
        break;
      }
      out += ch;
      w += cw;
    }
  }
  if (overflow) {
    out += '…';
    w += 1;
  }
  if (hadAnsi) out += '\x1b[0m';
  return out + ' '.repeat(Math.max(0, width - w));
}

function renderToggleRow(toggle, on) {
  const state = on ? '\x1b[30;42m ON  \x1b[0m' : '\x1b[97;41m OFF \x1b[0m';
  return `  \x1b[2m[tab]\x1b[0m ${toggle.label}  ${state}`;
}

// A keyed toggle with both labels is a choice between two options, not an
// on/off state — show both side by side with the active one highlighted.
// Only single-label toggles get the green/red ON/OFF treatment.
function renderKeyedRow(t) {
  let state;
  if (t.onLabel && t.offLabel) {
    const seg = (label, active) => (active ? `\x1b[7m ${label} \x1b[0m` : `\x1b[2m ${label} \x1b[0m`);
    state = seg(t.offLabel, !t.value) + '\x1b[2m│\x1b[0m' + seg(t.onLabel, t.value);
  } else {
    state = t.value ? `\x1b[30;42m ${t.onLabel || 'ON'} \x1b[0m` : `\x1b[97;41m ${t.offLabel || 'OFF'} \x1b[0m`;
  }
  return `  \x1b[2m[${t.key}]\x1b[0m ${t.label}  ${state}`;
}

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
    // Long lists scroll inside a viewport sized to the terminal — the repaint
    // moves the cursor up by a fixed row count, so it must never exceed the
    // screen height.
    const overhead = 2 + (toggle ? 1 : 0) + keyed.length; // message + hint (+ toggle rows)
    const visible = Math.max(3, Math.min(choices.length, (stdout.rows || 30) - overhead - 1));
    let offset = Math.max(0, Math.min(index - visible + 1, choices.length - visible));
    const lines = visible + 1 + (toggle ? 1 : 0) + keyed.length; // message + visible choices (+ toggle rows)
    const extraHints = [
      toggle ? 'tab toggle' : null,
      ...keyed.map((t) => `${t.key} ${t.shortLabel || 'toggle'}`)
    ].filter(Boolean);
    const hint = () => {
      const pos = choices.length > visible ? ` · ${index + 1}/${choices.length}` : '';
      return `\x1b[2m  ↑/↓ move · enter select${extraHints.length ? ' · ' + extraHints.join(' · ') : ''}${pos} · esc cancel\x1b[0m`;
    };

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();

    const toggleRow = () => renderToggleRow(toggle, on);
    const keyedRow = renderKeyedRow;

    const paint = (first) => {
      if (!first) {
        readline.cursorTo(stdout, 0);
        readline.moveCursor(stdout, 0, -lines);
      }
      readline.clearScreenDown(stdout);
      stdout.write(`\x1b[1m${message}\x1b[0m\n`);
      for (let i = offset; i < offset + visible; i++) {
        const c = choices[i];
        const label = c.label ?? c.name ?? String(c.value);
        const more =
          i === offset && offset > 0 ? ' \x1b[2m↑\x1b[0m'
          : i === offset + visible - 1 && i < choices.length - 1 ? ' \x1b[2m↓\x1b[0m'
          : '';
        // The highlighted row is inverse over the plain label — embedded ANSI
        // resets would cut the highlight short and hurt readability.
        stdout.write(i === index ? `\x1b[7m ❯ ${stripAnsi(label)} \x1b[0m${more}\n` : `   ${label}${more}\n`);
      }
      if (toggle) stdout.write(toggleRow() + '\n');
      for (const t of keyed) stdout.write(keyedRow(t) + '\n');
      stdout.write(hint());
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
        if (index < offset) offset = index;
        else if (index >= offset + visible) offset = index - visible + 1;
        paint(false);
      } else if (key.name === 'down' || key.name === 'j') {
        index = (index + 1) % choices.length;
        if (index < offset) offset = index;
        else if (index >= offset + visible) offset = index - visible + 1;
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

// Two-column selector: the left column lists the main choices, the right column
// live-previews the highlighted choice's children (scrolling the left column
// updates it). Children per choice can be:
//   - an array of { label, value }      shown immediately
//   - an async () => [{ label, value }] fetched lazily on first highlight,
//                                       with a "loading…" placeholder
//   - null / undefined / []             right column shows `detail` text instead
// `childValue` preselects the child whose value matches (e.g. the last-used model).
// A `{ divider: true }` entry renders as a rule across the left column and is
// skipped by the cursor. `color` (an ANSI code like '\x1b[32m') tints a row.
// `clearScreen` wipes the terminal before the first paint.
//
// Keys: ↑/↓ move in the focused column, →/← switch columns, enter selects.
// Enter on the left column takes the highlighted child as-is (or none).
// Resolves { ...choice, child: {label,value}|null, childFocused, toggleOn?, toggles? };
// rejects Error('cancelled') on Esc / Ctrl-C. `toggle`/`toggles` as in select().
// `banner` (multi-line string) is painted once above the picker and survives
// repaints — the cursor-up repaint math only covers the rows below it.
export function selectColumns({ message, choices, startIndex = 0, toggle = null, toggles = [], clearScreen = false, banner = null }) {
  if (!isInteractive) {
    return Promise.reject(new Error('A terminal (TTY) is required to choose interactively. Use --provider / --model instead.'));
  }
  return new Promise((resolve, reject) => {
    let index = Math.max(0, Math.min(startIndex, choices.length - 1));
    while (choices[index]?.divider && index < choices.length - 1) index++;
    let focus = 'left';
    let on = toggle ? Boolean(toggle.value) : false;
    const keyed = toggles.map((t) => ({ ...t, value: Boolean(t.value) }));
    let finished = false;

    const labelOf = (c) => (c.divider ? '' : c.label ?? c.name ?? String(c.value));

    // Per-choice child state. `lazy` children load on first highlight.
    const kids = choices.map((c) => ({
      status:
        c.divider ? 'none'
        : typeof c.children === 'function' ? 'lazy'
        : Array.isArray(c.children) && c.children.length ? 'ready'
        : 'none',
      items: Array.isArray(c.children) && !c.divider ? c.children : [],
      index: 0,
      offset: 0
    }));

    const applyChildStart = (i) => {
      const k = kids[i];
      const want = choices[i].childValue;
      // '' is "nothing remembered", not a value to match.
      if (want) k.index = Math.max(0, k.items.findIndex((x) => x.value === want));
    };
    kids.forEach((_, i) => applyChildStart(i));

    const ensureLoaded = (i) => {
      const k = kids[i];
      if (k.status !== 'lazy') return;
      k.status = 'loading';
      Promise.resolve()
        .then(() => choices[i].children())
        .then((items) => {
          k.items = Array.isArray(items) ? items : [];
          k.status = k.items.length ? 'ready' : 'none';
          applyChildStart(i);
        })
        .catch(() => {
          k.status = 'none';
        })
        .finally(() => {
          if (!finished && index === i) paint(false);
        });
    };

    // ---- layout: fixed row count so the repaint cursor-up math stays valid ----
    const cols = stdout.columns || 80;
    const bannerRows = banner ? banner.split('\n').length : 0;
    const overhead = 2 + bannerRows + (toggle ? 1 : 0) + keyed.length; // banner + message + hint (+ toggle rows)
    const tallest = Math.max(choices.length, ...kids.map((k) => k.items.length));
    const visible = Math.max(3, Math.min(tallest, (stdout.rows || 30) - overhead - 1));
    const lines = visible + 1 + (toggle ? 1 : 0) + keyed.length;
    // Left column hugs its widest label; the right column takes the rest.
    const leftW = Math.min(Math.max(...choices.map((c) => visWidth(labelOf(c))), 10) + 2, Math.floor((cols - 4) / 2));
    const rightW = Math.max(10, cols - leftW - 4);
    let leftOffset = 0;

    const clampOffset = (offset, idx, len) => {
      if (idx < offset) offset = idx;
      else if (idx >= offset + visible) offset = idx - visible + 1;
      return Math.max(0, Math.min(offset, len - visible));
    };

    const extraHints = [
      toggle ? 'tab toggle' : null,
      ...keyed.map((t) => `${t.key} ${t.shortLabel || 'toggle'}`)
    ].filter(Boolean);
    const hint = () => {
      const k = kids[index];
      // Positions count real rows only, not dividers.
      const selPos = choices.slice(0, index + 1).filter((c) => !c.divider).length;
      const selTotal = choices.filter((c) => !c.divider).length;
      const pos = [
        choices.length > visible ? `${selPos}/${selTotal}` : null,
        k.status === 'ready' && k.items.length > visible ? `→ ${k.index + 1}/${k.items.length}` : null
      ].filter(Boolean);
      return `\x1b[2m  ↑/↓ move · →/← column · enter select${extraHints.length ? ' · ' + extraHints.join(' · ') : ''}${pos.length ? ' · ' + pos.join(' ') : ''} · esc cancel\x1b[0m`;
    };

    // cell(): fixed-width column cell. The focused selection is inverse video
    // over the plain label (embedded ANSI resets would cut the highlight short);
    // the unfocused selection stays bold so both cursors are visible. `color`
    // tints the whole cell (skipped for the inverse row, which is loud enough).
    const cell = (raw, width, selected, focused, color = '') => {
      if (selected && focused) return `\x1b[7m${fit('❯ ' + stripAnsi(raw), width)}\x1b[0m`;
      if (selected) return `${color}\x1b[1m${fit('❯ ' + stripAnsi(raw), width)}\x1b[0m`;
      return color ? `${color}${fit('  ' + stripAnsi(raw), width)}\x1b[0m` : fit('  ' + raw, width);
    };

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();

    const paint = (first) => {
      if (first) {
        if (clearScreen) stdout.write('\x1b[2J\x1b[H');
        if (banner) stdout.write(banner + '\n');
      } else {
        readline.cursorTo(stdout, 0);
        readline.moveCursor(stdout, 0, -lines);
      }
      readline.clearScreenDown(stdout);
      stdout.write(`\x1b[1m${message}\x1b[0m\n`);
      const k = kids[index];
      leftOffset = clampOffset(leftOffset, index, choices.length);
      if (k.status === 'ready') k.offset = clampOffset(k.offset, k.index, k.items.length);
      for (let r = 0; r < visible; r++) {
        const li = leftOffset + r;
        const left =
          li >= choices.length ? ' '.repeat(leftW)
          : choices[li].divider ? `\x1b[2m${'─'.repeat(leftW)}\x1b[0m`
          : cell(labelOf(choices[li]), leftW, li === index, focus === 'left', choices[li].color);
        let right = '';
        if (k.status === 'ready') {
          const ri = k.offset + r;
          if (ri < k.items.length) right = cell(labelOf(k.items[ri]), rightW, ri === k.index, focus === 'right');
        } else if (r === 0) {
          right = k.status === 'none' ? `\x1b[2m  ${choices[index].detail || ''}\x1b[0m` : '\x1b[2m  loading…\x1b[0m';
        }
        stdout.write(`${left} \x1b[2m│\x1b[0m ${right}\n`);
      }
      if (toggle) stdout.write(renderToggleRow(toggle, on) + '\n');
      for (const t of keyed) stdout.write(renderKeyedRow(t) + '\n');
      stdout.write(hint());
    };

    const cleanup = () => {
      finished = true;
      stdin.removeListener('keypress', onKey);
      stdin.setRawMode(false);
      stdin.pause();
      stdout.write('\n');
    };

    const move = (dir) => {
      if (focus === 'left') {
        do {
          index = (index + dir + choices.length) % choices.length;
        } while (choices[index].divider);
        ensureLoaded(index);
      } else {
        const k = kids[index];
        k.index = (k.index + dir + k.items.length) % k.items.length;
      }
      paint(false);
    };

    const onKey = (str, key) => {
      if (!key) return;
      if (key.name === 'up' || key.name === 'k') move(-1);
      else if (key.name === 'down' || key.name === 'j') move(1);
      else if (key.name === 'right') {
        const k = kids[index];
        if (focus === 'left' && k.status === 'ready' && k.items.length) {
          focus = 'right';
          paint(false);
        }
      } else if (key.name === 'left') {
        if (focus === 'right') {
          focus = 'left';
          paint(false);
        }
      } else if (toggle && (key.name === 'tab' || key.name === 'space')) {
        on = !on;
        paint(false);
      } else if (key.name === 'return' || key.name === 'enter') {
        const k = kids[index];
        cleanup();
        const choice = { ...choices[index] };
        choice.child = k.status === 'ready' && k.items.length ? { ...k.items[k.index] } : null;
        choice.childFocused = focus === 'right';
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
    ensureLoaded(index);
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
