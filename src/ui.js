import readline from 'node:readline';

const stdin = process.stdin;
const stdout = process.stdout;

export const isInteractive = Boolean(stdin.isTTY && stdout.isTTY);

// If the process dies while a menu owns the screen (crash, SIGTERM), put the
// terminal back: cursor visible, autowrap on, cooked input. Armed by the
// selectors below while they're live, disarmed in their cleanup.
let restoreSeq = null;
process.on('exit', () => {
  if (!restoreSeq) return;
  stdout.write(restoreSeq);
  try { stdin.setRawMode(false); } catch {}
});

// ---------- ANSI-aware text measurement (for column layouts) ----------

const ANSI_RE = /\x1b\[[0-9;]*m/g;
const stripAnsi = (s) => String(s ?? '').replace(ANSI_RE, '');

// Match every whitespace-separated search term against both the visible label
// and the underlying value. This lets "claude sonnet" and a precise provider
// id such as "anthropic/claude-sonnet" work equally well. Choices whose value
// isn't a plain string (a session record, say) can offer `filterText` as the
// hidden half of the haystack. Dividers are group labels for the unfiltered
// list, so a search drops them rather than leaving rules over nothing.
export function filterChoices(choices, query) {
  const terms = String(query || '').trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return choices;
  return choices.filter((choice) => {
    if (choice?.divider) return false;
    const label = choice?.label ?? choice?.name ?? '';
    const extra = choice?.filterText ?? (typeof choice?.value === 'string' ? choice.value : '');
    const haystack = `${stripAnsi(label)} ${extra}`.toLocaleLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}

// Nearest non-divider row at or after `from`, searching in `dir`; falls back to
// the other direction so a divider at either end can't strand the cursor.
export function selectableIndex(items, from, dir = 1) {
  for (let i = from; i >= 0 && i < items.length; i += dir) if (!items[i]?.divider) return i;
  for (let i = from; i >= 0 && i < items.length; i -= dir) if (!items[i]?.divider) return i;
  return -1;
}

// Approximate terminal cell width — CJK and emoji occupy two columns,
// combining marks / joiners / variation selectors occupy none (they overlay
// or merge with the previous glyph, so counting them misaligns columns).
function charWidth(cp) {
  if (
    cp === 0x200b || cp === 0x200c || cp === 0x200d || cp === 0xfeff ||
    (cp >= 0x0300 && cp <= 0x036f) ||
    (cp >= 0x1ab0 && cp <= 0x1aff) ||
    (cp >= 0x1dc0 && cp <= 0x1dff) ||
    (cp >= 0x20d0 && cp <= 0x20ff) ||
    (cp >= 0xfe00 && cp <= 0xfe0f) ||
    (cp >= 0xfe20 && cp <= 0xfe2f)
  )
    return 0;
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
export function fit(s, width) {
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
// ↑/↓ (or k/j) move with wrap-around; pgup/pgdn page; home/end jump.
// Returns the chosen item, or throws Error('cancelled') on Esc / Ctrl-C.
// Optional `toggle` adds an on/off switch (flipped with Tab/Space) shown under the
// list — handy for things like "Skip permissions". `toggles` can add extra
// keyed switches, each returned by name in `toggles`. With `filterable`, typing
// narrows the list by label/value; Backspace edits and Esc clears the filter.
export function select({ message, choices, startIndex = 0, toggle = null, toggles = [], filterable = false }) {
  if (!isInteractive) {
    return Promise.reject(new Error('A terminal (TTY) is required to choose interactively. Use --provider / --model instead.'));
  }
  return new Promise((resolve, reject) => {
    const allChoices = choices;
    let items = allChoices;
    let query = '';
    let index = Math.max(0, Math.min(startIndex, items.length - 1));
    let on = toggle ? Boolean(toggle.value) : false;
    const keyed = toggles.map((t) => ({ ...t, value: Boolean(t.value) }));
    // Long lists scroll inside a viewport sized to the terminal — the repaint
    // moves the cursor up by the row count of the previous paint, so it must
    // never exceed the screen height. Recomputed on terminal resize.
    let visible, lines;
    const layout = () => {
      const overhead = 2 + (toggle ? 1 : 0) + keyed.length; // message + hint (+ toggle rows)
      // Reserve one result row for the "no matches" state. Otherwise never
      // paint more rows than the filtered list contains.
      visible = Math.min(Math.max(1, items.length), Math.max(3, (stdout.rows || 30) - overhead - 1));
      lines = visible + 1 + (toggle ? 1 : 0) + keyed.length; // message + visible choices (+ toggle rows)
    };
    layout();
    let offset = Math.max(0, Math.min(index - visible + 1, items.length - visible));
    let paintedLines = 0;
    const extraHints = [
      toggle ? 'tab toggle' : null,
      ...keyed.map((t) => `${t.key} ${t.shortLabel || 'toggle'}`)
    ].filter(Boolean);
    const hint = () => {
      const search = filterable
        ? query
          ? ` · filter: "${query}" ${items.length}/${allChoices.length} · backspace edit · esc clear`
          : ` · type to filter ${allChoices.length}`
        : '';
      const pos = items.length > visible ? ` · ${index + 1}/${items.length}` : '';
      return `\x1b[2m  ↑/↓ move · enter select${search}${extraHints.length ? ' · ' + extraHints.join(' · ') : ''}${pos} · esc cancel\x1b[0m`;
    };

    const applyFilter = () => {
      const selected = items[index]?.value;
      items = filterChoices(allChoices, query);
      const preserved = items.findIndex((choice) => choice.value === selected);
      index = preserved >= 0 ? preserved : 0;
      offset = 0;
      layout();
    };

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();
    restoreSeq = '\x1b[?25h\x1b[?7h';
    stdout.write('\x1b[?25l');

    const toggleRow = () => renderToggleRow(toggle, on);
    const keyedRow = renderKeyedRow;

    // mode: 'first' paints in place, 'repaint' overdraws the previous frame,
    // 'fresh' wipes the screen (after a resize the old rows have rewrapped and
    // the cursor-up math no longer holds). Autowrap is off while painting so a
    // row longer than the terminal truncates instead of wrapping — a wrapped
    // row would make the frame taller than `lines` and every repaint after it
    // would leave a stale copy of the menu behind.
    const paint = (mode) => {
      if (index < offset) offset = index;
      else if (index >= offset + visible) offset = index - visible + 1;
      offset = Math.max(0, Math.min(offset, items.length - visible));
      let out = '\x1b[?7l';
      if (mode === 'repaint') out += `\r\x1b[${paintedLines}A`;
      else if (mode === 'fresh') out += '\x1b[2J\x1b[H';
      out += '\x1b[0J';
      out += `\x1b[1m${message}\x1b[0m\n`;
      for (let i = offset; i < offset + visible; i++) {
        const c = items[i];
        if (!c) {
          out += `\x1b[2m   No matches for "${query}"\x1b[0m\n`;
          continue;
        }
        const label = c.label ?? c.name ?? String(c.value);
        const more =
          i === offset && offset > 0 ? ' \x1b[2m↑\x1b[0m'
          : i === offset + visible - 1 && i < items.length - 1 ? ' \x1b[2m↓\x1b[0m'
          : '';
        // The highlighted row is inverse over the plain label — embedded ANSI
        // resets would cut the highlight short and hurt readability.
        out += i === index ? `\x1b[7m ❯ ${stripAnsi(label)} \x1b[0m${more}\n` : `   ${label}${more}\n`;
      }
      if (toggle) out += toggleRow() + '\n';
      for (const t of keyed) out += keyedRow(t) + '\n';
      out += hint();
      out += '\x1b[?7h';
      stdout.write(out);
      paintedLines = lines;
    };

    // Terminals fire resize continuously while the window is dragged — a full
    // clear+repaint per event strobes. Coalesce to one repaint per pause.
    let resizeTimer = null;
    const onResize = () => {
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        resizeTimer = null;
        layout();
        paint('fresh');
      }, 80);
    };
    stdout.on('resize', onResize);

    const cleanup = () => {
      if (resizeTimer) clearTimeout(resizeTimer);
      stdin.removeListener('keypress', onKey);
      stdout.removeListener('resize', onResize);
      stdin.setRawMode(false);
      stdin.pause();
      restoreSeq = null;
      stdout.write('\x1b[?25h\x1b[?7h\n');
    };

    // Page/home/end jumps are clamped, not wrapped — wrapping past the end on
    // a page jump is disorienting. Arrow keys keep their wrap-around.
    const jump = (dir, step) => {
      if (!items.length) return;
      index = Math.max(0, Math.min(items.length - 1, index + dir * step));
      paint('repaint');
    };

    const onKey = (str, key) => {
      if (!key) return;
      const printable = !key.ctrl && !key.meta && typeof str === 'string' && [...str].length === 1 && str >= ' ';
      const reserved = !query && (
        key.name === 'j' ||
        key.name === 'k' ||
        keyed.some((item) => key.name === item.key || str === item.key) ||
        (toggle && key.name === 'space')
      );
      if (filterable && key.name === 'backspace' && query) {
        query = [...query].slice(0, -1).join('');
        applyFilter();
        paint('repaint');
      } else if (filterable && key.ctrl && key.name === 'u' && query) {
        query = '';
        applyFilter();
        paint('repaint');
      } else if (filterable && printable && !reserved) {
        query += str;
        applyFilter();
        paint('repaint');
      } else if ((key.name === 'up' || key.name === 'k') && items.length) {
        index = (index - 1 + items.length) % items.length;
        paint('repaint');
      } else if ((key.name === 'down' || key.name === 'j') && items.length) {
        index = (index + 1) % items.length;
        paint('repaint');
      } else if (key.name === 'pageup') {
        jump(-1, visible);
      } else if (key.name === 'pagedown') {
        jump(1, visible);
      } else if (key.name === 'home') {
        jump(-1, Infinity);
      } else if (key.name === 'end') {
        jump(1, Infinity);
      } else if (toggle && (key.name === 'tab' || key.name === 'space')) {
        on = !on;
        paint('repaint');
      } else if (key.name === 'return' || key.name === 'enter') {
        if (!items.length) {
          stdout.write('\x07');
          return;
        }
        cleanup();
        const choice = { ...items[index] };
        if (toggle) choice.toggleOn = on;
        if (keyed.length) choice.toggles = Object.fromEntries(keyed.map((t) => [t.name || t.key, t.value]));
        resolve(choice);
      } else if (key.name === 'escape' && filterable && query) {
        query = '';
        applyFilter();
        paint('repaint');
      } else if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        cleanup();
        reject(new Error('cancelled'));
      } else {
        const t = keyed.find((item) => key.name === item.key || str === item.key);
        if (t) {
          t.value = !t.value;
          paint('repaint');
        }
      }
    };

    stdin.on('keypress', onKey);
    paint('first');
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
// `filterableChildren` lets typing narrow that choice's children by label/value.
// A `{ divider: true }` entry renders as a rule across the left column and is
// skipped by the cursor. `color` (an ANSI code like '\x1b[32m') tints a row.
// `clearScreen` wipes the terminal before the first paint.
//
// Keys: ↑/↓ move in the focused column, pgup/pgdn page, home/end jump,
// →/← switch columns, enter selects.
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
    const kids = choices.map((c) => {
      const items = Array.isArray(c.children) && !c.divider ? c.children : [];
      return {
        status:
          c.divider ? 'none'
          : typeof c.children === 'function' ? 'lazy'
          : items.length ? 'ready'
          : 'none',
        allItems: items,
        items,
        query: '',
        index: 0,
        offset: 0
      };
    });

    const applyChildStart = (i) => {
      const k = kids[i];
      const want = choices[i].childValue;
      // '' is "nothing remembered", not a value to match.
      const found = want ? k.items.findIndex((x) => x.value === want) : -1;
      k.index = Math.max(0, selectableIndex(k.items, found >= 0 ? found : 0));
    };
    kids.forEach((_, i) => applyChildStart(i));

    const applyChildFilter = (i) => {
      const k = kids[i];
      const selected = k.items[k.index]?.value;
      k.items = filterChoices(k.allItems, k.query);
      const preserved = k.items.findIndex((item) => item.value === selected);
      k.index = Math.max(0, selectableIndex(k.items, preserved >= 0 ? preserved : 0));
      k.offset = 0;
    };

    // The highlighted child, or null when the column is empty / on a divider.
    const childAt = (k) => (k.status === 'ready' && !k.items[k.index]?.divider ? k.items[k.index] : null);

    const ensureLoaded = (i) => {
      const k = kids[i];
      if (k.status !== 'lazy') return;
      k.status = 'loading';
      Promise.resolve()
        .then(() => choices[i].children())
        .then((items) => {
          k.allItems = Array.isArray(items) ? items : [];
          k.items = filterChoices(k.allItems, k.query);
          k.status = k.allItems.length ? 'ready' : 'none';
          applyChildStart(i);
        })
        .catch(() => {
          k.status = 'none';
        })
        .finally(() => {
          if (!finished && index === i) {
            layout();
            paint('repaint');
          }
        });
    };

    // ---- layout: known row count so the repaint cursor-up math stays valid.
    // Recomputed on terminal resize (which triggers a fresh full repaint). ----
    let cols, visible, lines, leftW, rightW;
    const layout = () => {
      cols = stdout.columns || 80;
      const bannerRows = banner ? banner.split('\n').length : 0;
      const overhead = 2 + bannerRows + (toggle ? 1 : 0) + keyed.length; // banner + message + hint (+ toggle rows)
      const tallest = Math.max(choices.length, ...kids.map((k) => k.items.length));
      // At least 3 rows even on a tiny terminal, but never taller than the
      // tallest column — extra rows would just paint blank.
      visible = Math.min(tallest, Math.max(3, (stdout.rows || 30) - overhead - 1));
      lines = visible + 1 + (toggle ? 1 : 0) + keyed.length;
      // Left column hugs its widest label; the right column takes the rest.
      leftW = Math.min(Math.max(...choices.map((c) => visWidth(labelOf(c))), 10) + 2, Math.floor((cols - 4) / 2));
      rightW = Math.max(10, cols - leftW - 4);
    };
    layout();
    let leftOffset = 0;
    let paintedLines = 0;

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
      const kidPos = k.items.slice(0, k.index + 1).filter((c) => !c.divider).length;
      const kidTotal = k.items.filter((c) => !c.divider).length;
      const kidAll = k.allItems.filter((c) => !c.divider).length;
      const pos = [
        choices.length > visible ? `${selPos}/${selTotal}` : null,
        k.status === 'ready' && k.items.length > visible ? `→ ${kidPos}/${kidTotal}` : null
      ].filter(Boolean);
      const search = choices[index].filterableChildren
        ? k.query
          ? ` · filter: "${k.query}" ${kidTotal}/${kidAll} · backspace edit · esc clear`
          : ` · → then type to filter ${kidAll || ''}`.trimEnd()
        : '';
      return `\x1b[2m  ↑/↓ move · →/← column · enter select${search}${extraHints.length ? ' · ' + extraHints.join(' · ') : ''}${pos.length ? ' · ' + pos.join(' ') : ''} · esc cancel\x1b[0m`;
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

    // A divider row: a rule, or a labelled rule that names the group beneath it.
    const dividerCell = (label, width) => {
      const text = stripAnsi(label ?? '');
      if (!text) return `\x1b[2m${'─'.repeat(width)}\x1b[0m`;
      const rule = Math.max(0, width - visWidth(text) - 4);
      return `\x1b[2m── ${text} ${'─'.repeat(rule)}\x1b[0m`;
    };

    readline.emitKeypressEvents(stdin);
    stdin.setRawMode(true);
    stdin.resume();
    restoreSeq = '\x1b[?25h\x1b[?7h';
    stdout.write('\x1b[?25l');

    // mode: 'first' honours clearScreen/banner, 'repaint' overdraws the
    // previous frame, 'fresh' wipes the screen and repaints banner and all
    // (used after a resize, when the old rows have rewrapped and the cursor-up
    // math no longer holds). Autowrap is off while painting so a row longer
    // than the terminal truncates instead of wrapping — a wrapped row would
    // make the frame taller than `lines` and every repaint after it would
    // leave a stale copy of the menu behind.
    const paint = (mode) => {
      let out = '\x1b[?7l';
      if (mode === 'repaint') {
        out += `\r\x1b[${paintedLines}A`;
      } else {
        if (mode === 'fresh' || clearScreen) out += '\x1b[2J\x1b[H';
        if (banner) out += banner + '\n';
      }
      out += '\x1b[0J';
      out += `\x1b[1m${message}\x1b[0m\n`;
      const k = kids[index];
      leftOffset = clampOffset(leftOffset, index, choices.length);
      if (k.status === 'ready') k.offset = clampOffset(k.offset, k.index, k.items.length);
      for (let r = 0; r < visible; r++) {
        const li = leftOffset + r;
        const left =
          li >= choices.length ? ' '.repeat(leftW)
          : choices[li].divider ? dividerCell(choices[li].label, leftW)
          : cell(labelOf(choices[li]), leftW, li === index, focus === 'left', choices[li].color);
        let right = '';
        if (k.status === 'ready') {
          if (!k.items.length && r === 0) {
            right = `\x1b[2m  No matches for "${k.query}"\x1b[0m`;
          } else {
            const ri = k.offset + r;
            const item = k.items[ri];
            if (item) {
              right = item.divider
                ? dividerCell(item.label, rightW)
                : cell(labelOf(item), rightW, ri === k.index, focus === 'right', item.color);
            }
          }
        } else if (r === 0) {
          right = k.status === 'none' ? `\x1b[2m  ${choices[index].detail || ''}\x1b[0m` : '\x1b[2m  loading…\x1b[0m';
        }
        out += `${left} \x1b[2m│\x1b[0m ${right}\n`;
      }
      if (toggle) out += renderToggleRow(toggle, on) + '\n';
      for (const t of keyed) out += renderKeyedRow(t) + '\n';
      out += hint();
      out += '\x1b[?7h';
      stdout.write(out);
      paintedLines = lines;
    };

    // Terminals fire resize continuously while the window is dragged — a full
    // clear+repaint per event strobes. Coalesce to one repaint per pause.
    let resizeTimer = null;
    const onResize = () => {
      if (finished) return;
      if (resizeTimer) clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        resizeTimer = null;
        if (finished) return;
        layout();
        paint('fresh');
      }, 80);
    };
    stdout.on('resize', onResize);

    const cleanup = () => {
      finished = true;
      if (resizeTimer) clearTimeout(resizeTimer);
      stdin.removeListener('keypress', onKey);
      stdout.removeListener('resize', onResize);
      stdin.setRawMode(false);
      stdin.pause();
      restoreSeq = null;
      stdout.write('\x1b[?25h\x1b[?7h\n');
    };

    const move = (dir) => {
      if (focus === 'left') {
        do {
          index = (index + dir + choices.length) % choices.length;
        } while (choices[index].divider);
        ensureLoaded(index);
      } else {
        const k = kids[index];
        // Wrap around the child list, stepping over group dividers. The counter
        // bounds the walk so a list of nothing but dividers can't spin forever.
        for (let n = 0; n < k.items.length; n++) {
          k.index = (k.index + dir + k.items.length) % k.items.length;
          if (!k.items[k.index]?.divider) break;
        }
      }
      layout();
      paint('repaint');
    };

    // Page/home/end jump in the focused column. Clamped, not wrapped —
    // wrapping past the end on a page jump is disorienting. If the landing
    // row is a divider, keep going in the same direction (or back off it at
    // a list boundary).
    const jump = (dir, step) => {
      if (focus === 'left') {
        let i = Math.max(0, Math.min(choices.length - 1, index + dir * step));
        while (choices[i].divider && i + dir >= 0 && i + dir < choices.length) i += dir;
        while (choices[i].divider) i -= dir;
        index = i;
        ensureLoaded(index);
      } else {
        const k = kids[index];
        if (k.items.length) {
          const landed = Math.max(0, Math.min(k.items.length - 1, k.index + dir * step));
          k.index = Math.max(0, selectableIndex(k.items, landed, dir));
        }
      }
      layout();
      paint('repaint');
    };

    const onKey = (str, key) => {
      if (!key) return;
      const k = kids[index];
      const filterable = Boolean(choices[index].filterableChildren);
      const printable = !key.ctrl && !key.meta && typeof str === 'string' && [...str].length === 1 && str >= ' ';
      const reserved = !k.query && focus === 'left' && (
        key.name === 'j' ||
        key.name === 'k' ||
        keyed.some((item) => key.name === item.key || str === item.key) ||
        (toggle && key.name === 'space')
      );
      if (filterable && key.name === 'backspace' && k.query) {
        k.query = [...k.query].slice(0, -1).join('');
        applyChildFilter(index);
        layout();
        paint('repaint');
      } else if (filterable && key.ctrl && key.name === 'u' && k.query) {
        k.query = '';
        applyChildFilter(index);
        layout();
        paint('repaint');
      } else if (filterable && printable && !reserved) {
        k.query += str;
        focus = 'right';
        applyChildFilter(index);
        layout();
        paint('repaint');
      } else if (key.name === 'up' || key.name === 'k') move(-1);
      else if (key.name === 'down' || key.name === 'j') move(1);
      else if (key.name === 'pageup') jump(-1, visible);
      else if (key.name === 'pagedown') jump(1, visible);
      else if (key.name === 'home') jump(-1, Infinity);
      else if (key.name === 'end') jump(1, Infinity);
      else if (key.name === 'right') {
        if (focus === 'left' && k.status === 'ready' && k.items.length) {
          focus = 'right';
          paint('repaint');
        }
      } else if (key.name === 'left') {
        if (focus === 'right') {
          focus = 'left';
          paint('repaint');
        }
      } else if (toggle && (key.name === 'tab' || key.name === 'space')) {
        on = !on;
        paint('repaint');
      } else if (key.name === 'return' || key.name === 'enter') {
        if (filterable && focus === 'right' && !k.items.length) {
          stdout.write('\x07');
          return;
        }
        cleanup();
        const choice = { ...choices[index] };
        const child = childAt(k);
        choice.child = child ? { ...child } : null;
        choice.childFocused = focus === 'right';
        if (toggle) choice.toggleOn = on;
        if (keyed.length) choice.toggles = Object.fromEntries(keyed.map((t) => [t.name || t.key, t.value]));
        resolve(choice);
      } else if (key.name === 'escape' && filterable && k.query) {
        k.query = '';
        applyChildFilter(index);
        layout();
        paint('repaint');
      } else if (key.name === 'escape' || (key.ctrl && key.name === 'c')) {
        cleanup();
        reject(new Error('cancelled'));
      } else {
        const t = keyed.find((item) => key.name === item.key || str === item.key);
        if (t) {
          t.value = !t.value;
          paint('repaint');
        }
      }
    };

    stdin.on('keypress', onKey);
    ensureLoaded(index);
    paint('first');
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
        stdin.removeListener('keypress', onKey);
        stdin.setRawMode(false);
        stdin.pause();
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
