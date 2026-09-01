const ANSI_RE = /\x1b\[[0-9;]*m/g;

const cellWidth = (value) => [...String(value ?? '').replace(ANSI_RE, '')].length;

// One box-drawing renderer so every `bro` report looks like the same tool.
// `numeric` right-aligns those column indexes, `rules` draws a divider above
// the listed row indexes — that is how subtotal and total bands are marked.
export function table({ headers, rows, numeric = [], rules = [] }) {
  const widths = headers.map((_, column) =>
    Math.max(...[headers, ...rows].map((row) => cellWidth(row[column]))));
  const pad = (cell, column) => {
    const value = String(cell ?? '');
    const fill = ' '.repeat(Math.max(0, widths[column] - cellWidth(value)));
    return numeric.includes(column) ? fill + value : value + fill;
  };
  const divider = (left, middle, right) =>
    left + widths.map((width) => '─'.repeat(width + 2)).join(middle) + right;
  const render = (row) => `│ ${row.map(pad).join(' │ ')} │`;

  const lines = [divider('┌', '┬', '┐'), render(headers), divider('├', '┼', '┤')];
  rows.forEach((row, index) => {
    if (rules.includes(index)) lines.push(divider('├', '┼', '┤'));
    lines.push(render(row));
  });
  lines.push(divider('└', '┴', '┘'));
  return lines.join('\n');
}

// A proportion drawn as a fixed-width bar. Partial eighth-blocks give a row
// eight times the resolution of its cell count, and anything above zero keeps
// at least a sliver so a small-but-real account never reads as an empty one.
const EIGHTHS = ['', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

export function bar(fraction, width = 10) {
  const share = Number.isFinite(fraction) ? Math.min(Math.max(fraction, 0), 1) : 0;
  const eighths = Math.round(share * width * 8);
  const drawn = '█'.repeat(Math.floor(eighths / 8)) + EIGHTHS[eighths % 8];
  const filled = share > 0 && drawn === '' ? EIGHTHS[1] : drawn;
  return filled + '░'.repeat(Math.max(0, width - [...filled].length));
}
