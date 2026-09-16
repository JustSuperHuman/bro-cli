// Codex (ChatGPT subscription) — the Codex half of bro's account switcher.
//
// It mirrors the Claude side (pool.js) one for one: several logins, each an
// isolated home directory, listed with the sessions they can resume, and a
// session from one login can be forked into another. Where Claude switches by
// setting CLAUDE_CONFIG_DIR, Codex switches by setting CODEX_HOME — the shared
// picker rows, resume-destination prompt and staging live in profiles.js.
//
// Three harness paths can run against a Codex login:
//
//   claude / omp / pi — bro ensures the login (its own PKCE flow in codex-auth.js;
//     an existing codex CLI login is reused), fetches that subscription's live
//     model list, starts the local Anthropic⇄Codex translation bridge
//     (codex-bridge.js), points the harness at it via ANTHROPIC_BASE_URL, and
//     tears the bridge down when the harness exits.
//
//   codex — the codex CLI runs directly under that login's CODEX_HOME. No
//     bridge, and no model for bro to choose: codex has its own picker.
//
// Resuming a session always runs the codex CLI, whatever the harness toggle
// says, because a Codex rollout is a codex conversation and only codex can
// read it back.

import fs from 'node:fs';
import { permissionArgs } from './launch.js';
import path from 'node:path';
import { ensureClaude, runInherit } from './proc.js';
import { select, selectColumns, prompt } from './ui.js';
import { rememberSelection, rememberProfile, lastModelFor } from './state.js';
import { isCodexLoggedIn, codexLogin, codexLogout, codexAuthStatus } from './codex-auth.js';
import { fetchCodexModels, startCodexBridge, DEFAULT_PORT } from './codex-bridge.js';
import { launchCodex, launchOmp, launchPi } from './launch.js';
import { launchDsh } from './deepseek.js';
import { note } from './out.js';
import { prepareClaudeBrowser } from './claude-browser.js';
import { listCodexSessions } from './codex-sessions.js';
import { samePath } from './sessions.js';
import { chooseResumeProfile, sessionRows, stageFiles } from './profiles.js';
import {
  codexHomeFor,
  codexProfileLabel,
  codexSessionEntries,
  createCodexProfile,
  defaultCodexHome,
  importCodexProfile,
  listCodexProfiles,
  localCodexProfile,
  removeCodexProfile,
  profilesDir
} from './codex-profiles.js';

export const CODEX_PROVIDER = {
  id: 'codex',
  name: 'Codex (ChatGPT subscription)',
  mode: 'codex',
  models: []
};

const LOCAL_LABEL = "This machine's Codex login";

// A profile names a directory on disk (where its rollouts live, and what a
// fork has to be staged into) — but the *local* login is deliberately not a
// directory bro passes anywhere: leaving CODEX_HOME alone is what keeps the
// user's own environment in charge, and lets bro's own credential file stand
// in when the codex CLI has never been signed in.
const dirOf = (profile) => codexHomeFor(profile);
const homeOf = (profile) => (profile ? codexHomeFor(profile) : '');

// Every login a session listing should cover: the machine's own, then each
// stored profile.
const sessionSources = () => [
  { account: null, home: defaultCodexHome() },
  ...listCodexProfiles().map((p) => ({ account: p.name, home: p.dir }))
];

// --- picker rows -----------------------------------------------------------

// The Codex provider's right pane: the logins you can launch under (the
// machine's own first, then profiles), a way into the full profile menu, and
// below them the sessions those logins can resume — this project's first, then
// everything else with its path, all reachable by typing to filter.
export async function codexProfileChoices() {
  const local = localCodexProfile();
  const profiles = listCodexProfiles();
  const rows = [
    { label: codexProfileLabel(local, { name: LOCAL_LABEL }), value: '' },
    ...profiles.map((p) => ({ label: codexProfileLabel(p), value: p.name })),
    { label: 'Log in / manage Codex profiles…', value: { manage: true } }
  ];

  let sessions = [];
  try {
    sessions = await listCodexSessions({ sources: sessionSources() });
  } catch {
    // Session history is a convenience — never let it cost you the provider.
    return rows;
  }
  return [...rows, ...sessionRows(sessions, (s) => ({
    kind: 'codex-session',
    id: s.id,
    account: s.account,
    cwd: s.cwd,
    title: s.title,
    file: s.file
  }))];
}

// The direct `bro codex` route: the same combined profile/session list, on its
// own rather than in the provider picker.
async function chooseCodexTarget() {
  const choice = await selectColumns({
    message: 'Choose a Codex login or session:',
    choices: [{
      label: CODEX_PROVIDER.name,
      detail: 'chatgpt login',
      children: codexProfileChoices,
      filterableChildren: true
    }]
  }).catch(() => null);

  if (!choice) return null;
  const value = choice.child?.value;
  if (value && typeof value === 'object') {
    if (value.kind === 'codex-session') return { profile: '', session: value };
    return { profile: '', session: null, manage: true };
  }
  return { profile: typeof value === 'string' ? value : '', session: null };
}

// --- profile management ----------------------------------------------------

// The full menu behind "Log in / manage Codex profiles…" and `bro codex login`
// with no name: pick a login to use, or add / import / remove one. Loops until
// something is chosen, so a login lands you back in the list.
// Resolves { name } for a login ('' is the machine's own), or null to cancel.
async function chooseCodexProfile(preferredName = '') {
  if (preferredName) {
    const found = listCodexProfiles().find((p) => p.name === preferredName);
    if (!found) throw new Error(`Unknown Codex profile: ${preferredName}. Run "bro codex profiles" to see them.`);
    if (!found.authenticated && !(await loginProfile(found.name))) return null;
    return { name: found.name };
  }

  while (true) {
    const local = localCodexProfile();
    const profiles = listCodexProfiles();
    const choices = [
      { label: codexProfileLabel(local, { name: LOCAL_LABEL }), value: { action: 'use', name: '' } },
      ...profiles.map((p) => ({
        label: codexProfileLabel(p),
        value: p.authenticated ? { action: 'use', name: p.name } : { action: 'login', name: p.name }
      })),
      { label: 'Log in / add another ChatGPT account', value: { action: 'login' } },
      { label: "Import this machine's current Codex login", value: { action: 'import' } },
      ...(profiles.length ? [{ label: 'Remove a Codex profile', value: { action: 'remove' } }] : []),
      { label: 'Cancel', value: { action: 'cancel' } }
    ];

    const choice = await select({ message: 'Choose a Codex profile:', choices })
      .catch(() => ({ value: { action: 'cancel' } }));
    const picked = choice.value;

    if (picked.action === 'cancel') return null;
    if (picked.action === 'use') return { name: picked.name };
    if (picked.action === 'login') {
      const fallback = picked.name || 'work';
      const name = picked.name || (await prompt(`Profile name [${fallback}]: `).catch(() => '')) || fallback;
      if (await loginProfile(name)) return { name };
    } else if (picked.action === 'import') {
      const name = (await prompt('Name for the imported profile [primary]: ').catch(() => '')) || 'primary';
      try {
        importCodexProfile(name);
        console.log(`Imported this machine's Codex login as "${name}".`);
        return { name };
      } catch (e) {
        console.error(e.message);
      }
    } else if (picked.action === 'remove') {
      await removeProfileInteractive(profiles);
    }
  }
}

// Sign one profile in with bro's own ChatGPT flow, into that profile's
// auth.json — the same file the codex CLI reads when it runs there.
async function loginProfile(name) {
  try {
    createCodexProfile(name);
    await codexLogin({ home: codexHomeFor(name) });
    return true;
  } catch (e) {
    console.error(`Login failed: ${e.message}`);
    return false;
  }
}

async function removeProfileInteractive(profiles) {
  const choice = await select({
    message: 'Remove which Codex profile? (its sessions and login are deleted)',
    choices: [
      ...profiles.map((p) => ({ label: codexProfileLabel(p), value: p.name })),
      { label: 'Cancel', value: '' }
    ]
  }).catch(() => ({ value: '' }));
  if (!choice.value) return;
  const answer = await prompt(`Type the profile name to confirm deleting "${choice.value}": `).catch(() => '');
  if (answer.trim() !== choice.value) {
    console.log('Left it alone.');
    return;
  }
  console.log(removeCodexProfile(choice.value) ? `Removed Codex profile "${choice.value}".` : 'That profile was already gone.');
}

// --- commands --------------------------------------------------------------

function printProfiles() {
  const local = localCodexProfile();
  console.log(`\n  ${codexProfileLabel(local, { name: LOCAL_LABEL })}`);
  console.log(`  \x1b[2m${local.source || defaultCodexHome()}\x1b[0m`);
  const profiles = listCodexProfiles();
  if (!profiles.length) {
    console.log('\n  No Codex profiles yet. Add one:  bro codex login <name>\n');
    return;
  }
  for (const p of profiles) {
    console.log(`\n  ${codexProfileLabel(p)}`);
    console.log(`  \x1b[2m${p.dir}\x1b[0m`);
  }
  console.log(`\n  \x1b[2mProfiles live in ${profilesDir()}\x1b[0m\n`);
}

// `bro codex <login|logout|status|profiles|import|remove|resume>` — login and
// session management without going through the provider picker.
export async function runCodexCommand(args = [], { skipPermissions = true } = {}) {
  const sub = args[0];
  const name = args[1];

  // Bare `bro codex` opens the combined login/session list, the way bare
  // `bro account` does for Claude.
  if (sub == null) {
    const target = await chooseCodexTarget();
    if (!target) { console.log('Cancelled.'); return 0; }
    return runCodex({ ...target, harness: 'codex', chooseProfile: true, skipPermissions });
  }
  if (sub === 'resume' || sub === 'sessions') {
    const session = name
      ? { id: name, account: null, cwd: '' }
      : (await chooseCodexTarget())?.session;
    if (!session) { console.log('Cancelled.'); return 0; }
    return runCodex({ session, skipPermissions });
  }
  if (sub === 'profiles' || sub === 'list' || sub === 'accounts') {
    printProfiles();
    return 0;
  }
  if (sub === 'import') {
    if (!name) { console.error('Usage: bro codex import <name>'); return 1; }
    try {
      importCodexProfile(name);
      console.log(`Imported this machine's Codex login as "${name}".`);
      return 0;
    } catch (e) {
      console.error(e.message);
      return 1;
    }
  }
  if (sub === 'remove' || sub === 'rm') {
    if (!name) { console.error('Usage: bro codex remove <name>'); return 1; }
    console.log(removeCodexProfile(name) ? `Removed Codex profile "${name}".` : `No Codex profile named "${name}".`);
    return 0;
  }
  if (sub === 'logout') {
    const home = name ? codexHomeFor(name) : '';
    const removed = codexLogout(home);
    console.log(
      removed
        ? `Logged out of ${name ? `Codex profile "${name}"` : 'the ChatGPT subscription (bro credentials removed)'}.`
        : `No stored credentials for ${name ? `profile "${name}"` : 'bro'}.`
    );
    return 0;
  }
  if (sub === 'status') {
    const status = codexAuthStatus(name ? codexHomeFor(name) : '');
    const who = name ? `Codex profile "${name}"` : "This machine's Codex login";
    if (!status.loggedIn) {
      console.log(`${who}: not logged in. Run: bro codex login${name ? ' ' + name : ''}`);
      return 0;
    }
    console.log(`${who}: logged in${status.plan ? ` (${status.plan} plan)` : ''}.`);
    console.log(`  credentials: ${status.source}`);
    return 0;
  }
  if (sub === 'login') {
    // With a name, sign that profile in. Without one, keep the original
    // meaning: bro's own credentials for the machine's login.
    if (name) return (await loginProfile(name)) ? 0 : 1;
    try {
      await codexLogin();
      return 0;
    } catch (e) {
      console.error(`Login failed: ${e.message}`);
      return 1;
    }
  }
  // Anything else names a profile to launch under, the way `bro account work`
  // does for Claude.
  try {
    const target = await chooseCodexProfile(sub);
    if (!target) { console.log('Cancelled.'); return 0; }
    return await runCodex({ profile: target.name, harness: 'codex', skipPermissions });
  } catch (e) {
    console.error(e.message);
    return 1;
  }
}

// --- launching -------------------------------------------------------------

// Make sure the chosen login has credentials, offering to sign it in when it
// doesn't. Returns the profile name to run under, or null when cancelled.
async function ensureLogin(profile) {
  if (isCodexLoggedIn(homeOf(profile))) return profile;
  console.log(`\nNo ChatGPT login found for ${profile ? `profile "${profile}"` : 'this machine'}.`);
  const choice = await select({
    message: 'Log in to ChatGPT to use Codex models?',
    choices: [
      { label: 'Log in now (opens ChatGPT in your browser)', value: 'login' },
      { label: 'Choose another Codex profile', value: 'switch' },
      { label: 'Cancel', value: 'cancel' }
    ]
  }).catch(() => ({ value: 'cancel' }));

  if (choice.value === 'switch') {
    const target = await chooseCodexProfile();
    return target ? ensureLogin(target.name) : null;
  }
  if (choice.value !== 'login') return null;
  if (profile) return (await loginProfile(profile)) ? profile : null;
  try {
    await codexLogin();
    return profile;
  } catch (e) {
    console.error(`Login failed: ${e.message}`);
    return null;
  }
}

async function chooseModel(models, skip) {
  const lastM = lastModelFor(CODEX_PROVIDER.id);
  const choice = await select({
    message: 'Choose a model for Codex:',
    startIndex: lastM != null ? Math.max(0, models.findIndex((m) => m.id === lastM)) : 0,
    choices: models.map((m) => ({ label: `${m.name}  \x1b[2m(${m.id})\x1b[0m`, value: m.id })),
    toggle: { label: 'Skip permissions', value: skip }
  }).catch(() => null);
  return choice;
}

// Which login should resume this session: its owner by default, any other
// profile at the cost of a fork.
async function resumeTarget(session) {
  const target = await chooseResumeProfile({
    session,
    profiles: listCodexProfiles().map((p) => ({ name: p.name, label: codexProfileLabel(p) })),
    message: 'Choose the Codex profile to resume this session with:',
    localLabel: LOCAL_LABEL,
    manageLabel: 'Log in / manage Codex profiles…'
  });
  if (!target) return null;
  if (!target.manage) return target.name;
  const picked = await chooseCodexProfile();
  return picked ? picked.name : null;
}

export async function runCodex({
  model = '',
  harness = 'claude',
  profile = '',
  session = null,
  manage = false,
  chooseProfile = false,
  extraArgs = [],
  permissionMode,
  skipPermissions = permissionMode ? permissionMode === 'bypass' : true,
  providers = [],
  providerKeys = {},
  headless = false,
  dryRun = false
} = {}) {
  // A Codex session is a codex-CLI rollout: only codex can read it, so
  // resuming one runs the codex CLI whatever the harness toggle says. The same
  // CLI is what the codex harness launches, minus the resume.
  const runCli = Boolean(session) || harness === 'codex';

  if (dryRun) {
    const target = profile || session?.account || '';
    const fork = Boolean(session) && !samePath(dirOf(session.account || ''), dirOf(target));
    if (runCli) {
      return {
        ...(await launchCodex({
          provider: CODEX_PROVIDER,
          model,
          extraArgs,
          skipPermissions,
          home: homeOf(target),
          resume: session?.id || '',
          resumeTitle: session?.title || '',
          fork,
          cwd: session?.cwd || '',
          dryRun: true
        })),
        profile: profile || session?.account || '(this machine)',
        ...(session ? { sourceProfile: session.account || '(this machine)', forkSession: fork } : {})
      };
    }
    const models = await fetchCodexModels({ home: homeOf(target) });
    const bridgeProvider = {
      ...CODEX_PROVIDER,
      mode: 'anthropic',
      baseUrl: `http://127.0.0.1:${DEFAULT_PORT}`,
      disable1mContext: true,
      models: models.map((entry) => ({ id: entry.id, name: entry.name }))
    };
    const out = {
      via: 'codex (chatgpt subscription) → local bridge → ' + (harness === 'omp' ? 'omp' : harness === 'pi' ? 'pi' : harness === 'dsh' ? 'DeepSeek Harness' : 'claude'),
      profile: profile || '(this machine)',
      auth: isCodexLoggedIn(homeOf(target)) ? 'logged in' : 'not logged in (login would run)',
      bridge: `http://127.0.0.1:${DEFAULT_PORT}  (Anthropic-compatible)`,
      model: model || '(menu)',
      models: models.map((m) => m.id),
      harness
    };
    if (harness === 'dsh') {
      out.dsh = await launchDsh({
        provider: bridgeProvider,
        model,
        apiKey: 'bro-codex',
        providers: [bridgeProvider, ...providers],
        providerKeys: { ...providerKeys, [bridgeProvider.id]: 'bro-codex' },
        extraArgs,
        skipPermissions,
        dryRun: true,
        reuseCodex: {
          home: homeOf(target),
          provider: bridgeProvider,
          models: bridgeProvider.models
        }
      });
    }
    return out;
  }

  // The picker's "Log in / manage…" row opens the full profile menu.
  if (manage) {
    const target = await chooseCodexProfile();
    if (!target) { console.log('Cancelled.'); return 0; }
    profile = target.name;
  }

  // A session defaults to the login that owns it; choosing another stages the
  // rollout there and forks, so neither copy overwrites the original. Only a
  // session picked from the switcher raises the question — one named by id on
  // the command line means "this one, here".
  let staged = null;
  if (session) {
    const destination = profile || (chooseProfile ? await resumeTarget(session) : session.account || '');
    if (destination === null) { console.log('Cancelled.'); return 0; }
    profile = destination;
  }

  // A login that turns out to be signed out can send the user to another
  // profile, so where this runs is only settled once that has resolved.
  const loggedIn = await ensureLogin(profile);
  if (loggedIn === null) { console.log('Cancelled.'); return 0; }
  profile = loggedIn;

  const home = homeOf(profile);
  const sourceDir = session ? dirOf(session.account || '') : '';
  const fork = Boolean(session) && !samePath(sourceDir, dirOf(profile));

  // A session picked from the list knows its rollout file; one named by id on
  // the command line doesn't, and codex looks that up itself.
  if (session?.file && !fs.existsSync(session.file)) {
    console.error(`\nThat session's rollout is gone: ${session.file}`);
    return 1;
  }

  // The codex CLI carries its own ChatGPT login and model picker, so it needs
  // neither bro's model menu nor the bridge.
  if (runCli) {
    // Resuming is a one-off jump back into an old conversation — it shouldn't
    // rewrite the provider/model the picker opens on next time. A headless
    // run is a one-off for the same reason: a script doing a job, not someone
    // choosing what to use from now on.
    if (!session && !headless) {
      rememberSelection(CODEX_PROVIDER.id, model, harness);
      rememberProfile(CODEX_PROVIDER.id, profile);
    }
    try {
      if (fork) staged = stageFiles({
        targetRoot: dirOf(profile),
        entries: codexSessionEntries(session, { sourceHome: sourceDir, targetHome: dirOf(profile) })
      });
      return await launchCodex({
        provider: CODEX_PROVIDER,
        model,
        extraArgs,
        skipPermissions,
        home,
        profile,
        resume: session?.id || '',
        resumeTitle: session?.title || '',
        sourceProfile: session?.account || '',
        fork,
        cwd: session?.cwd && session.cwd !== process.cwd() ? session.cwd : '',
        dryRun: false
      });
    } catch (e) {
      console.error(`\n${e.message}`);
      return 1;
    } finally {
      staged?.cleanup();
    }
  }

  process.stderr.write('\x1b[2mFetching Codex models…\x1b[0m\r');
  const models = await fetchCodexModels({ home });
  process.stderr.write('\x1b[2K\r');

  let skip = skipPermissions;
  // Claude Code and Pi need a concrete bridge model. omp does its own model
  // routing, so it alone can skip bro's picker.
  if (!model && harness !== 'omp' && headless) {
    // Nothing to pick with: take the subscription's first model, the row the
    // picker would have opened on.
    model = models[0]?.id || '';
    if (models.length > 1) note(`\x1b[2mNo -m given; using ${model || 'the default model'}.\x1b[0m`);
  } else if (!model && harness !== 'omp') {
    const choice = await chooseModel(models, skip);
    if (choice == null) { console.log('Cancelled.'); return 0; }
    model = choice.value;
    if (choice.toggleOn !== undefined) skip = choice.toggleOn;
  }
  const activeModel = model || models[0]?.id || '';

  // Start the local bridge on this login's credentials.
  let bridge;
  try {
    bridge = await startCodexBridge({ defaultModel: activeModel, models, home });
  } catch (e) {
    console.error(`Could not start the Codex bridge: ${e.message}`);
    return 1;
  }

  if (!headless) {
    rememberSelection(CODEX_PROVIDER.id, model, harness);
    rememberProfile(CODEX_PROVIDER.id, profile);
  }

  const stop = () => bridge.close().catch(() => {});
  try {
    if (harness === 'omp') {
      return await launchOmp({
        provider: {
          ...CODEX_PROVIDER,
          mode: 'anthropic',
          baseUrl: bridge.baseUrl,
          disable1mContext: true,
          models: models.map((m) => ({ id: m.id, name: m.name }))
        },
        model,
        apiKey: 'bro-codex',
        extraArgs,
        skipPermissions: skip,
        dryRun: false
      });
    }
    if (harness === 'pi') {
      return await launchPi({
        provider: {
          ...CODEX_PROVIDER,
          mode: 'anthropic',
          baseUrl: bridge.baseUrl,
          disable1mContext: true,
          models: models.map((entry) => ({ id: entry.id, name: entry.name }))
        },
        model: activeModel,
        apiKey: 'bro-codex',
        extraArgs,
        dryRun: false
      });
    }
    if (harness === 'dsh') {
      const bridgeProvider = {
        ...CODEX_PROVIDER,
        mode: 'anthropic',
        baseUrl: bridge.baseUrl,
        disable1mContext: true,
        models: models.map((entry) => ({ id: entry.id, name: entry.name }))
      };
      return await launchDsh({
        provider: bridgeProvider,
        model: activeModel,
        apiKey: 'bro-codex',
        providers: [bridgeProvider, ...providers],
        providerKeys: { ...providerKeys, [bridgeProvider.id]: 'bro-codex' },
        extraArgs,
        skipPermissions: skip,
        dryRun: false,
        reuseCodex: {
          home: homeOf(profile),
          provider: bridgeProvider,
          models: bridgeProvider.models
        }
      });
    }

    const { claude, dirs } = ensureClaude();

    const env = { ...process.env };
    delete env.CLAUDE_CONFIG_DIR;
    delete env.ANTHROPIC_API_KEY;
    env.ANTHROPIC_BASE_URL = bridge.baseUrl;
    env.ANTHROPIC_AUTH_TOKEN = 'bro-codex';
    env.CLAUDE_CODE_DISABLE_1M_CONTEXT = '1';
    env.NODE_NO_WARNINGS = '1';
    env.PATH = [...dirs, env.PATH || ''].join(path.delimiter);

    // Claude runs against the local Codex bridge here, so its own browser
    // wiring is off (that needs a claude.ai login) and the browser arrives
    // through the MCP server instead.
    const browser = extraArgs.includes('--no-chrome') || extraArgs.includes('--chrome')
      ? null
      : await prepareClaudeBrowser({ claudePath: claude, baseEnv: env, skipPermissions: skip, autoStart: !headless });
    if (browser) Object.assign(env, browser.env);

    const claudeArgs = permissionArgs(skip ? 'bypass' : permissionMode === 'auto' ? 'auto' : 'manual');
    claudeArgs.push(...(browser?.args || []));
    if (activeModel) claudeArgs.push('--model', activeModel);
    claudeArgs.push(...extraArgs);

    note(
      `\nLaunching Claude Code on ${profile ? `Codex profile "${profile}"` : 'your ChatGPT subscription'}${activeModel ? ' / ' + activeModel : ''}…`
    );
    note(`\x1b[2m  bridge: ${bridge.baseUrl}\x1b[0m\n`);
    return await runInherit(claude, claudeArgs, env, { terminalAgent: 'claude' });
  } finally {
    stop();
  }
}
