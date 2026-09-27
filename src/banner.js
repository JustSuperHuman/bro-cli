// The top of bro's pickers: the logo, and beside it the Usage section — what
// each app has left across all of its accounts (see usage.js):
//
//    ___  ___  ____     _______   ____          ╭─ Usage ─────────────────────────────────╮
//   / _ )/ _ \/ __ \   / ___/ /  /  _/          │        5h   week              5h   week │
//  / _  / , _/ /_/ /  / /__/ /___/ /            │ ✻     58%    49%    Fable    58%     2% │
// /____/_/|_|\____/   \___/____/___/  (⌐■_■)    │ >_      —    16%                        │
//                                               ╰─────────────────────────────────────────╯
//
// It's a function of the terminal width, repainted with the picker, so the
// figures fill in as each account answers. Where the box doesn't fit beside
// the logo it goes underneath; with no signed-in account it's left out.

import { usageLeftSection } from './account-usage.js';
import { visWidth } from './ui.js';

// Quirky BRO CLI logo. Cyan fade, shades on.
const LOGO = [
  '\x1b[96m   ___  ___  ____     _______   ____\x1b[0m',
  '\x1b[96m  / _ )/ _ \\/ __ \\   / ___/ /  /  _/\x1b[0m',
  '\x1b[36m / _  / , _/ /_/ /  / /__/ /___/ /\x1b[0m',
  '\x1b[36m/____/_/|_|\\____/   \\___/____/___/\x1b[0m  (⌐■_■)'
];
const GAP = 4;
const DIM = '\x1b[2m';
const BOLD = '\x1b[1m';
const RESET = '\x1b[0m';

// Lines inside a rounded box titled `title`, dim borders and a bold title.
// A `caption` sits in the bottom edge the way the title sits in the top one.
export function titledBox(title, lines, { caption = '' } = {}) {
  const inner = Math.max(...lines.map(visWidth), visWidth(title) + 2, caption ? visWidth(caption) + 2 : 0);
  const border = (text) => `${DIM}${text}${RESET}`;
  const bottom = caption
    ? `${border('╰─')} ${DIM}${caption}${RESET} ${border(`${'─'.repeat(inner - visWidth(caption) - 1)}╯`)}`
    : border(`╰${'─'.repeat(inner + 2)}╯`);
  return [
    `${border('╭─')} ${BOLD}${title}${RESET} ${border(`${'─'.repeat(inner - visWidth(title) - 1)}╮`)}`,
    ...lines.map((line) => `${border('│')} ${line}${' '.repeat(inner - visWidth(line))} ${border('│')}`),
    bottom
  ];
}

// The banner for a picker. `usage` is what account-usage.js requestAllUsage()
// returned (null for the logo alone).
export function brandBanner(usage) {
  const logoWidth = Math.max(...LOGO.map(visWidth));
  return (width = Infinity) => {
    const section = usage ? usageLeftSection(usage) : { lines: [], notes: [] };
    const lines = section.lines.map((line) => line());
    const box = lines.length ? titledBox('Usage', lines, { caption: section.notes.join(' · ') }) : [];
    let rows;
    if (!box.length) rows = LOGO;
    else if (logoWidth + GAP + visWidth(box[0]) <= width) {
      rows = Array.from({ length: Math.max(LOGO.length, box.length) }, (_, i) => {
        const logo = LOGO[i] || '';
        return box[i] ? `${logo}${' '.repeat(logoWidth - visWidth(logo) + GAP)}${box[i]}` : logo;
      });
    } else rows = [...LOGO, ...box];
    // A blank row between the banner and the picker's title.
    return [...rows, ''].join('\n');
  };
}
