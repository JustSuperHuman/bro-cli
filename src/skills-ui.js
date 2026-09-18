import fs from 'node:fs';
import path from 'node:path';
import { isInteractive, select } from './ui.js';
import { note } from './out.js';
import { scanSkills } from './skills.js';
import { consolidate } from './skills-consolidator.js';

const GREEN = '\x1b[32m';
const RED = '\x1b[31m';
const DIM = '\x1b[2m';
const RESET = '\x1b[0m';
const BOLD = '\x1b[1m';

const exists = (target) => { try { fs.lstatSync(target); return true; } catch { return false; } };

function guardPaths(root) {
  const resolved = fs.realpathSync(path.resolve(root));
  const links = [];
  for (const directory of ['.claude', '.codex']) {
    const parent = path.join(resolved, directory, 'skills');
    if (!exists(parent)) continue;
    for (const name of fs.readdirSync(parent)) {
      const target = path.join(parent, name);
      if (fs.lstatSync(target).isSymbolicLink()) links.push(path.resolve(fs.realpathSync(target)));
    }
  }
  if (links.some((link) => !link.startsWith(resolved + path.sep))) {
    throw new Error('A linked skill resolves outside the scanned root. Consolidation is limited to links inside this root.');
  }
  if (fs.realpathSync(resolved) === fs.realpathSync(path.dirname(resolved))) {
    throw new Error('Refusing to consolidate the filesystem root.');
  }
}

function summary(scan) {
  const ready = scan.candidates.filter((candidate) => candidate.status === 'ready').length;
  const blocked = scan.candidates.filter((candidate) => candidate.status === 'blocked').length;
  const synced = scan.candidates.filter((candidate) => candidate.status === 'synced').length;
  return `${ready} ready · ${blocked} blocked · ${synced} already synced`;
}

function reasonFor(candidate) {
  if (candidate.status === 'blocked') {
    return `${RED}${candidate.reason || 'Not safe to move.'}${RESET}`;
  }
  if (candidate.status === 'synced') return `${DIM}Already linked.${RESET}`;
  return `${GREEN}Ready to consolidate.${RESET}`;
}

function table(scan) {
  if (!scan.candidates.length) return `${DIM}No skills found in .claude, .codex or .agents.${RESET}`;
  const width = Math.max(...scan.candidates.map((candidate) => candidate.name.length), 10);
  const mark = (candidate, tool) => {
    const entry = candidate.entries.find((item) => item.tool === tool);
    if (!entry) return `${DIM}    ·${RESET}`;
    if (entry.kind === 'skill') return `${GREEN}copy${RESET}`;
    if (entry.kind === 'link') return `${GREEN}link${RESET}`;
    return `${RED}  ! ${RESET}`;
  };
  const status = (candidate) => candidate.status === 'ready'
    ? `${GREEN}ready${RESET}`
    : candidate.status === 'blocked'
      ? `${RED}blocked${RESET}`
      : `${DIM}synced${RESET}`;
  const rows = scan.candidates.map((candidate) =>
    `${candidate.name.padEnd(width)}  ${[...['claude', 'codex', 'agents']].map((tool) => mark(candidate, tool)).join('   ')}   ${status(candidate)}\n    ${DIM}${candidate.reason || ''}${RESET}`
  );
  return [
    `${BOLD}${'skill'.padEnd(width)}   claude  codex  agents   status${RESET}`,
    `${DIM}${'─'.repeat(width + 46)}${RESET}`,
    ...rows
  ].join('\n');
}

export function runSkillsCommand(args = [], { root = process.cwd() } = {}) {
  const [command] = args;
  if (command === 'consolidate') {
    const names = args.slice(1).filter((arg) => !arg.startsWith('--'));
    const options = {
      rootName: args.includes('--dry-run') ? '.bro-skills-history-dry-run' : '.bro-skills-history'
    };
    if (args.includes('--undo')) {
      try {
        const result = consolidate(root, [], { ...options, undo: true });
        note(`Undid ${result.undone.length} skill${result.undone.length === 1 ? '' : 's'}: ${result.undone.join(', ') || 'none'}`);
        return 0;
      } catch (error) {
        note(error.message);
        return 1;
      }
    }
    const scan = scanSkills(root);
    if (!scan.candidates.length) {
      note('No skills found in .claude, .codex or .agents. Nothing to do.');
      return 0;
    }
    if (args.includes('--dry-run')) {
      console.log(table(scan));
      console.log('');
      note('No changes were made.');
      return 0;
    }
    guardPaths(root);
    if (!args.includes('--yes')) {
      note('Consolidation moves files and replaces them with links.');
      note('Dry-run preview:');
      console.log(table(scan));
      note('Add --yes to apply these changes.');
      return 0;
    }
    try {
      const result = consolidate(root, names, options);
      note(`Consolidated ${result.operations.length} skill${result.operations.length === 1 ? '' : 's'} into ${scan.directories.agents}. Undo with: bro skills consolidate --undo`);
      return 0;
    } catch (error) {
      note(error.message);
      return 1;
    }
  }
  if (command === 'help' || command === '-h' || command === '--help') {
    console.log(`bro skills — inspect and consolidate Claude/Codex skills

Usage:
  bro skills                  Open the interactive Skills menu
  bro skills consolidate      Preview first; add --yes to apply
  bro skills consolidate --dry-run
                              Print the preview and move nothing
  bro skills consolidate --undo
                              Restore the last consolidation, if unchanged
                              (requires no later changes to the canonical skills)
`);
    return 0;
  }
  if (!isInteractive) {
    note('A terminal (TTY) is required for the interactive Skills menu. Use `bro skills consolidate --dry-run` instead.');
    return 1;
  }
  return runSkillsMenu({ root });
}

async function runSkillsMenu({ root }) {
  let scan = scanSkills(root);
  let action = null;
  while (action !== 'exit') {
    const choice = await select({
      message: 'Skills:',
      filterable: true,
      choices: [
        { label: 'Skills Consolidator — preview changes', value: 'preview' },
        {
          label: `${GREEN}Consolidate now${RESET}`,
          value: 'consolidate',
          detail: summary(scan)
        },
        {
          label: 'Preview changes',
          value: 'preview',
          detail: scan.candidates.length ? `${GREEN}${scan.candidates.filter((candidate) => candidate.status === 'ready').length} ready${RESET}` : ''
        },
        {
          label: 'Undo last consolidation',
          value: 'undo',
          detail: exists(path.join(root, '.bro-skills-history')) ? `${DIM}Available${RESET}` : `${DIM}No history yet.${RESET}`
        },
        { label: 'Rescan', value: 'rescan' },
        { label: 'Exit', value: 'exit' }
      ]
    }).catch((error) => {
      if (error.message === 'cancelled') return null;
      throw error;
    });
    if (!choice) return 0;
    action = choice.value;
    if (action === 'rescan') scan = scanSkills(root);
    if (action === 'preview') console.log(table(scan));
    if (action === 'consolidate') {
      guardPaths(root);
      try {
        const result = consolidate(root, [], { undo: false });
        note(`Consolidated ${result.operations.length} skills.`);
        scan = scanSkills(root);
      } catch (error) {
        note(error.message);
      }
    }
    if (action === 'undo') {
      try {
        const result = consolidate(root, [], { undo: true });
        note(`Undid ${result.undone.length} skills.`);
        scan = scanSkills(root);
      } catch (error) {
        note(error.message);
      }
    }
  }
  return 0;
}
