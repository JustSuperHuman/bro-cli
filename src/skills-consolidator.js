import fs from 'node:fs';
import path from 'node:path';
import { scanSkills, skillDigest } from './skills.js';

const exists = (target) => { try { fs.lstatSync(target); return true; } catch { return false; } };

function removeDirectory(target) {
  fs.rmSync(target, { recursive: true, force: true });
}

function createLink(target, linkPath) {
  fs.mkdirSync(path.dirname(linkPath), { recursive: true });
  fs.symlinkSync(path.relative(path.dirname(linkPath), target), linkPath, process.platform === 'win32' ? 'junction' : 'dir');
}

function linkPointsTo(linkPath, target) {
  try {
    return fs.lstatSync(linkPath).isSymbolicLink()
      && fs.realpathSync(linkPath) === fs.realpathSync(target);
  } catch { return false; }
}

function removeGuardedLink(linkPath, canonical) {
  if (!exists(linkPath)) return;
  if (!linkPointsTo(linkPath, canonical)) {
    throw new Error(`Refusing to remove ${linkPath}; it is no longer this consolidation's link.`);
  }
  fs.unlinkSync(linkPath);
}

function restoreBackup(backup, destination) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  if (exists(destination)) removeDirectory(destination);
  fs.cpSync(backup, destination, { recursive: true });
  fs.rmSync(backup, { recursive: true, force: true });
}

function rollbackOperation(historyPath, operation, canonicalRoot) {
  const canonical = path.join(canonicalRoot, operation.name);
  for (const source of [...operation.sources].reverse()) {
    const destination = path.resolve(canonicalRoot, '..', '..', source.tool, 'skills', source.name);
    fs.mkdirSync(path.dirname(destination), { recursive: true });
    if (source.action === 'linked') {
      removeGuardedLink(destination, canonical);
      if (source.backup) restoreBackup(path.join(historyPath, source.backup), destination);
    } else if (source.action === 'moved') {
      if (exists(canonical) && linkPointsTo(canonical, canonical)) fs.unlinkSync(canonical);
      removeGuardedLink(path.resolve(canonicalRoot, '..', '..', '.claude', 'skills', operation.name), canonical);
      if (exists(canonical)) fs.renameSync(canonical, destination);
    }
  }
  removeDirectory(path.join(historyPath, operation.name));
}

function undoHistory(root, historyPath) {
  if (!exists(historyPath)) throw new Error('No skills history found; there is nothing to undo.');
  const history = JSON.parse(fs.readFileSync(path.join(historyPath, 'history.json'), 'utf8'));
  const canonicalRoot = path.resolve(root, history.root || '.', '.agents', 'skills');
  for (const operation of history.operations) {
    const canonical = path.join(canonicalRoot, operation.name);
    if (!exists(canonical) || skillDigest(canonical) !== operation.digest) {
      throw new Error(`Cannot undo ${operation.name}: its canonical skill is missing or changed.`);
    }
  }
  for (const operation of [...history.operations].reverse()) {
    rollbackOperation(historyPath, operation, canonicalRoot);
  }
  removeDirectory(historyPath);
  return history.operations.map((operation) => operation.name).reverse();
}

export function consolidate(root, names = [], { undo = false, rootName = '.bro-skills-history' } = {}) {
  const rootPath = fs.realpathSync(path.resolve(root));
  const historyPath = path.join(rootPath, rootName);
  if (undo) return { undone: undoHistory(rootPath, historyPath) };

  const scan = scanSkills(rootPath);
  const requested = new Set(names.map((name) => name.toLowerCase()));
  const selected = names.length
    ? scan.candidates.filter((candidate) => requested.has(candidate.name.toLowerCase()))
    : scan.candidates.filter((candidate) => candidate.status === 'ready');
  if (names.length && selected.length !== names.length) {
    throw new Error(`Some requested skills are not ready: ${names.join(', ')}`);
  }

  if (exists(historyPath)) {
    throw new Error(`Skills history already exists at ${historyPath}. Undo it before consolidating again.`);
  }
  fs.mkdirSync(historyPath, { recursive: true });
  const history = { root: path.relative(rootPath, path.dirname(path.dirname(scan.directories.agents))) || '.', operations: [] };

  try {
    for (const candidate of selected) {
      if (candidate.status !== 'ready') throw new Error(`${candidate.name}: ${candidate.reason}`);
      const canonical = candidate.target;
      fs.mkdirSync(path.dirname(canonical), { recursive: true });
      const canonicalEntry = candidate.entries.find((entry) => entry.tool === 'agents' && entry.kind === 'skill');
      const canonicalExisted = exists(canonical);
      if (canonicalExisted && !canonicalEntry) throw new Error(`${candidate.name}: canonical skill missing.`);
      if (canonicalExisted && skillDigest(canonical) !== canonicalEntry.digest) {
        throw new Error(`${candidate.name}: canonical skill changed after scan.`);
      }

      const sources = candidate.entries
        .filter((entry) => entry.kind === 'skill' && (!canonicalExisted || entry.tool !== 'agents'))
        .map((entry) => ({ ...entry, digest: skillDigest(entry.path) }));
      const expected = canonicalExisted ? canonicalEntry.digest : sources[0]?.digest;
      if (!expected || sources.some((source) => source.digest !== expected)) {
        throw new Error(`${candidate.name}: skills changed or are not identical after scan.`);
      }

      const operation = { name: candidate.name, digest: expected, sources: [] };
      fs.mkdirSync(path.join(historyPath, candidate.name), { recursive: true });
      history.operations.push(operation);
      const record = (source) => operation.sources.push(source);

      if (canonicalExisted) {
        for (const source of sources) {
          const backup = path.join(candidate.name, `${source.tool}.skills`);
          record({ tool: `.${source.tool}`, name: source.name, action: 'linked', backup });
          fs.cpSync(source.path, path.join(historyPath, backup), { recursive: true });
          removeDirectory(source.path);
          createLink(canonical, source.path);
        }
      } else {
        const primary = sources.find((source) => source.tool === 'claude') || sources[0];
        record({ tool: `.${primary.tool}`, name: primary.name, action: 'moved' });
        fs.mkdirSync(path.dirname(canonical), { recursive: true });
        fs.renameSync(primary.path, canonical);
        for (const source of sources.filter((entry) => entry !== primary)) {
          const backup = path.join(candidate.name, `${source.tool}.skills`);
          record({ tool: `.${source.tool}`, name: source.name, action: 'linked', backup });
          fs.cpSync(source.path, path.join(historyPath, backup), { recursive: true });
          removeDirectory(source.path);
          createLink(canonical, source.path);
        }
      }

      for (const tool of ['claude', 'codex']) {
        const linkPath = path.join(scan.directories[tool], candidate.name);
        if (!exists(linkPath)) {
          record({ tool: `.${tool}`, name: candidate.name, action: 'linked' });
          createLink(canonical, linkPath);
        }
      }

      const seen = new Set(operation.sources.map((source) => `${source.tool}:${source.name}`));
      operation.sources = operation.sources.filter((source) => {
        const key = `${source.tool}:${source.name}`;
        if (seen.has(key)) {
          seen.delete(key);
          return true;
        }
        return false;
      });

      fs.writeFileSync(path.join(historyPath, 'history.json'), JSON.stringify(history, null, 2));
    }
    return { operations: history.operations, historyPath };
  } catch (error) {
    for (const operation of [...history.operations].reverse()) {
      try { rollbackOperation(historyPath, operation, scan.directories.agents); } catch { /* preserve the original failure */ }
    }
    try { removeDirectory(historyPath); } catch { /* best-effort cleanup */ }
    throw error;
  }
}
