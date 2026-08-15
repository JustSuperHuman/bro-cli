// Republish models.json to the Tigris bucket behind https://m.justgains.com.
//
// `bro` reads its model list from a remote copy (src/models.js REMOTE_URL), so a
// models.json change is invisible to users until it is BOTH pushed to GitHub and
// uploaded here. Those two steps drifted apart once already: claude-fable-5 was
// committed but never republished, so the hosted list sat a model behind for
// weeks. This script exists to make the upload half hard to forget and hard to
// get wrong.
//
// Usage: bun run deploy [--dry-run] [--force]
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const LOCAL = path.join(ROOT, 'models.json');

const BUCKET = process.env.BRO_MODELS_BUCKET || 't3://justgains/models.json';
const PUBLIC_URL = process.env.BRO_MODELS_PUBLIC_URL || 'https://m.justgains.com/models.json';
const GITHUB_RAW = 'https://raw.githubusercontent.com/JustSuperHuman/bro-cli/main/models.json';

const args = process.argv.slice(2);
const dryRun = args.includes('--dry-run');
const force = args.includes('--force');

const ok = (m) => console.log(`\x1b[32m✓\x1b[0m ${m}`);
const warn = (m) => console.log(`\x1b[33m!\x1b[0m ${m}`);
const step = (m) => console.log(`\n\x1b[1m${m}\x1b[0m`);

function fail(message, hint) {
  console.error(`\x1b[31m✗ ${message}\x1b[0m`);
  if (hint) console.error(`  ${hint}`);
  process.exitCode = 1;
  throw new Error('deploy aborted');
}

function git(...a) {
  const r = spawnSync('git', a, { cwd: ROOT, encoding: 'utf8' });
  return { code: r.status, out: (r.stdout || '').trim(), err: (r.stderr || '').trim() };
}

// The same guard fetchRemote() applies before caching a response: anything that
// isn't a providers list would poison every client that pulls it.
function validate(raw, source) {
  let json;
  try {
    json = JSON.parse(raw);
  } catch (e) {
    fail(`${source} is not valid JSON: ${e.message}`);
  }
  if (!json || !Array.isArray(json.providers) || !json.providers.length) {
    fail(`${source} has no "providers" array — refusing to publish`);
  }
  return json;
}

const summarize = (json) =>
  `${json.providers.length} providers, ${json.providers.reduce((n, p) => n + (p.models?.length || 0), 0)} models`;

async function main() {
  step('Validating models.json');
  const raw = fs.readFileSync(LOCAL);
  const local = validate(raw.toString('utf8'), 'models.json');
  ok(`${summarize(local)}, ${raw.length} bytes`);

  // Publishing a file that isn't committed and pushed recreates the same drift
  // in the other direction: the CDN would lead GitHub, and the next `git
  // checkout` would silently revert what users are being served.
  step('Checking git sync');
  if (git('diff', '--quiet', 'HEAD', '--', 'models.json').code !== 0) {
    if (!force) fail('models.json has uncommitted changes', 'Commit and push it first, or re-run with --force.');
    warn('models.json has uncommitted changes (--force)');
  } else {
    ok('models.json matches HEAD');
  }

  git('fetch', 'origin', 'main', '--quiet');
  const remote = git('show', 'origin/main:models.json');
  if (remote.code !== 0) {
    warn('could not read origin/main:models.json — skipping push check');
  } else if (remote.out !== raw.toString('utf8').trim()) {
    if (!force) fail('models.json differs from origin/main', 'Push it first so both sources agree, or re-run with --force.');
    warn('models.json differs from origin/main (--force)');
  } else {
    ok('models.json matches origin/main');
  }

  if (dryRun) {
    step('Dry run — nothing uploaded');
    console.log(`  would upload ${LOCAL}\n            -> ${BUCKET} (public)`);
    return;
  }

  // -a public is REQUIRED. A plain `tigris cp` uploads the object private and
  // does not inherit the previous ACL, which takes m.justgains.com to 403
  // AccessDenied for every client until it is re-uploaded.
  step('Uploading to Tigris');
  // No shell: true — the args would be concatenated unescaped (DEP0190), and the
  // CLI is a real executable that spawns fine without one.
  const up = spawnSync('tigris', ['cp', '-a', 'public', LOCAL, BUCKET], { cwd: ROOT, encoding: 'utf8' });
  if (up.error?.code === 'ENOENT') {
    fail('tigris CLI not found on PATH', 'Install it (https://www.tigrisdata.com/docs/sdks/cli/) and run `tigris login`.');
  }
  if (up.status !== 0) {
    fail(`tigris cp failed (exit ${up.status})`, (up.stderr || up.stdout || '').trim() || 'Is the tigris CLI authenticated? Try `tigris whoami`.');
  }
  ok(`uploaded -> ${BUCKET}`);

  step('Verifying published copy');
  const res = await fetch(PUBLIC_URL, { cache: 'no-store', headers: { accept: 'application/json', connection: 'close' } });
  if (!res.ok) {
    fail(`${PUBLIC_URL} returned HTTP ${res.status}`, res.status === 403 ? 'The object is private — the -a public flag did not apply.' : undefined);
  }
  const body = await res.text();
  validate(body, PUBLIC_URL);
  if (body !== raw.toString('utf8')) {
    fail('published copy does not match models.json', 'The upload may have raced a CDN cache — re-run to confirm.');
  }
  ok(`HTTP ${res.status} · ${res.headers.get('content-type')} · identical to models.json`);

  // GitHub raw is the default REMOTE_URL, so it is the copy most clients pull.
  // It updates on push, not from here — report it so any gap is visible now
  // rather than the next time someone wonders where their model went.
  step('Cross-checking GitHub raw');
  try {
    const gh = await fetch(GITHUB_RAW, { cache: 'no-store', headers: { connection: 'close' } });
    const text = await gh.text();
    if (text.trim() === raw.toString('utf8').trim()) ok('GitHub raw matches — both sources in sync');
    else warn(`GitHub raw differs from models.json — push to main (${GITHUB_RAW})`);
  } catch {
    warn('could not reach GitHub raw — skipped');
  }

  console.log('\n\x1b[32mDeployed.\x1b[0m Clients pick it up on `bro update` or a cold cache.');
}

main().catch((err) => {
  if (err?.message !== 'deploy aborted') {
    console.error(`\x1b[31m✗ ${err?.message || err}\x1b[0m`);
    process.exitCode = 1;
  }
});
