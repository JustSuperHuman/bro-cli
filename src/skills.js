import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

const TOOL_NAMES = ['.claude', '.codex', '.agents'];

function statOrNull(target) {
  try { return fs.lstatSync(target); } catch (error) {
    if (error.code === 'ENOENT') return null;
    throw error;
  }
}

export function skillDigest(directory) {
  const hash = createHash('sha256');
  const walk = (current, relative = '') => {
    for (const name of fs.readdirSync(current).sort()) {
      const target = path.join(current, name);
      const stat = fs.lstatSync(target);
      const entry = path.posix.join(relative, name);
      if (stat.isSymbolicLink()) throw new Error(`Contains a nested link (${entry}); relocation could break it.`);
      if (!stat.isDirectory() && !stat.isFile()) throw new Error(`Contains a special file (${entry}).`);
      hash.update(JSON.stringify([entry, stat.isDirectory() ? 'directory' : 'file', stat.mode & 0o111]));
      if (stat.isDirectory()) walk(target, entry);
      else hash.update(createHash('sha256').update(fs.readFileSync(target)).digest());
    }
  };
  walk(directory);
  return hash.digest('hex');
}

function discoverDirectory(parent, expected) {
  if (!statOrNull(parent)) return path.join(parent, expected);
  const matches = fs.readdirSync(parent).filter((name) => name.toLowerCase() === expected);
  if (matches.length > 1) throw new Error(`Multiple case variants of ${expected} in ${parent}; merge or rename them first.`);
  const target = path.join(parent, matches[0] || expected);
  const stat = statOrNull(target);
  if (stat && (stat.isSymbolicLink() || !stat.isDirectory())) {
    throw new Error(`${target} is linked or is not a directory; scan its real root instead.`);
  }
  return target;
}

export function scanSkills(root) {
  const resolved = fs.realpathSync(path.resolve(root));
  if (!fs.statSync(resolved).isDirectory()) throw new Error('The skills root must be a directory.');
  const directories = Object.fromEntries(TOOL_NAMES.map((tool) => {
    const home = discoverDirectory(resolved, tool);
    return [tool.slice(1), discoverDirectory(home, 'skills')];
  }));
  const grouped = new Map();
  const ignored = [];
  for (const [tool, directory] of Object.entries(directories)) {
    if (!statOrNull(directory)) continue;
    for (const name of fs.readdirSync(directory).sort()) {
      const target = path.join(directory, name);
      if (name.startsWith('.')) {
        ignored.push({ path: target, reason: 'Hidden or tool-managed entry; left untouched.' });
        continue;
      }
      const entry = { tool, name, path: target, kind: 'blocked' };
      try {
        const stat = fs.lstatSync(target);
        if (stat.isSymbolicLink()) {
          entry.kind = 'link';
          entry.link = fs.readlinkSync(target);
          try { entry.realPath = fs.realpathSync(target); } catch { entry.problem = 'Broken link; repair or remove it manually.'; }
        } else if (stat.isDirectory() && statOrNull(path.join(target, 'SKILL.md'))?.isFile()) {
          entry.digest = skillDigest(target);
          entry.kind = 'skill';
        } else {
          entry.problem = 'Not a skill directory with a regular SKILL.md; left untouched.';
        }
      } catch (error) { entry.problem = error.message; }
      const key = name.toLowerCase();
      if (!grouped.has(key)) grouped.set(key, []);
      grouped.get(key).push(entry);
    }
  }
  const statusLabels = { ready: 'Ready to consolidate', blocked: 'Needs attention', synced: 'Already synced' };
  const candidates = [...grouped.values()].map((entries) => {
    const name = entries.find((entry) => entry.tool === 'agents')?.name || entries[0].name;
    const target = path.join(directories.agents, name);
    const canonical = entries.find((entry) => entry.tool === 'agents' && entry.kind === 'skill');
    let reason = entries.find((entry) => entry.problem)?.problem;
    if (new Set(entries.map((entry) => entry.name)).size > 1) reason = 'Case-colliding names; rename them explicitly for cross-platform compatibility.';
    if (entries.some((entry) => entry.kind === 'link' && (entry.tool === 'agents' || !canonical || entry.realPath !== fs.realpathSync(canonical.path)))) {
      reason ||= 'Existing link points outside this canonical skill; left untouched.';
    }
    const skills = entries.filter((entry) => entry.kind === 'skill');
    if (!reason && new Set(skills.map((entry) => entry.digest)).size > 1) reason = 'Different versions share this name. Compare the paths and rename one skill to keep both, then rescan.';
    const status = reason ? 'blocked' : canonical && ['claude', 'codex'].every((tool) => entries.some((entry) => entry.tool === tool && entry.kind === 'link')) ? 'synced' : 'ready';
    return { name, target, entries, status, reason: reason || statusLabels[status] };
  }).sort((left, right) => left.name.localeCompare(right.name));
  return { root: resolved, directories, candidates, ignored };
}
