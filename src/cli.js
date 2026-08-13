import { readFileSync } from 'node:fs';
import { loadConfig, ensureDefaultConfig, setKey, CONFIG_PATH } from './config.js';
import { loadModels, loadOpenRouterModels, mergeProviders, updateModels, REMOTE_URL } from './models.js';
import { select, selectColumns, promptHidden, isInteractive } from './ui.js';
import { launch } from './launch.js';
import { runPool, runPoolAccounts, runAccountProfile, accountProfileChoices, POOL_PROVIDER, ACCOUNT_PROVIDER } from './pool.js';
import { runImageGen, imageHelp, mergeImageApis, IMAGE_PROVIDER } from './imagegen.js';
import { runCodex, runCodexCommand, codexProfileChoices, CODEX_PROVIDER } from './codex.js';
import { runTokenReport } from './token-report.js';
import { rememberSelection, rememberHarness, lastProvider, lastModelFor, lastProfileFor, lastHarness } from './state.js';

const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

const HELP = `bro — run your preferred coding harness against any provider/model.

Usage:
  bro                    Pick a provider, then a model (interactive)
  bro -p pool            Multiple Claude Account Proxy — pool many Claude
                         plans, then launch Claude Code across them
  bro account [name]     Pick/run one logged-in Claude account profile
                         (sessions are listed too; choose their resume profile)
  bro accounts list      List pool accounts
  bro accounts login <name>
                         Add/log in a Claude account for the pool
  bro accounts import <name>
                         Copy this machine's current Claude login into the pool
  bro image              Image generation — pick an API, then a self-hosted
                         web UI opens (images save to ./.bro/image-gen)
  bro image -p <api>     Skip the image API menu (e.g. bro image -p yunwu)
  bro image help         Image-generation help (APIs, models, file paths)
  bro -p codex           Run Claude Code on your ChatGPT subscription — logs
                         in, fetches the live model list, and bridges through a
                         local Anthropic-compatible server (no codex CLI needed)
                         (Codex profiles and their sessions are listed too)
  bro codex [name]       Pick a Codex profile or session (like bro account),
                         or run the codex CLI under the named profile
  bro codex profiles     List Codex login profiles
  bro codex login [name] Log in a ChatGPT account (name = a profile of its own)
  bro codex import <name>
                         Copy this machine's Codex login into a profile
  bro codex remove <name>
                         Delete a Codex profile (its login and sessions)
  bro codex resume [id]  Resume a Codex session (picker when no id given)
  bro codex status [name]
                         Show login status for a profile / this machine
  bro codex logout [name]
                         Remove stored ChatGPT credentials
  bro tokens             Lifetime tokens for all Claude profiles + Codex
  bro -p <provider>      Skip the provider menu (id or name)
  bro --account <name>   Launch with a logged-in profile (Claude, or the Codex
                         profile of that name with -p codex)
  bro -m <model>         Skip the model menu (use with -p)
  bro --harness <name>   Choose harness: claude (default), omp, pi or codex
  bro --omp              Launch with omp instead of Claude Code; bro sets up
                         the provider and omp picks the model (-m overrides)
  bro --pi               Launch with Pi; bro sets up the provider/model and
                         passes API credentials only through the environment
  bro --codex            Launch the codex CLI instead of Claude Code (your
                         ChatGPT login, or a provider serving OpenAI's
                         Responses API)
  bro -l, --list         List every provider and model
  bro update             Refresh the model list from GitHub and cache it
  bro --dry-run          Show what would run; launch nothing
  bro --safe             Don't pass --dangerously-skip-permissions
  bro help, -h, --help   Show this help (bro help image for image help)
  bro -v, --version      Show version
  bro --resume <id>      Pick provider/model, then pass args to the harness
  bro -- <args...>       Force everything after -- straight to the harness

Put bro flags first. The first unrecognized arg, and everything after it,
is passed verbatim to the selected harness after provider/model selection.
Missing harnesses are installed automatically on first use.

Config:  ${CONFIG_PATH}
Models:  ${REMOTE_URL}
Docs:    https://justgains.com`;

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const t = argv[i];
    if (t === '--') { a._.push(...argv.slice(i + 1)); break; }
    if (t === '--provider' || t === '-p') a.provider = argv[++i];
    // --account names a login profile. On its own that means the Claude
    // account switcher; alongside -p it just names the profile (Codex has
    // them too), so an explicit provider is never overridden.
    else if (t === '--account') { a.account = argv[++i]; a.provider = a.provider || 'account'; }
    else if (t === '--model' || t === '-m') a.model = argv[++i];
    else if (t === '--harness') a.harness = argv[++i];
    else if (t === '--omp') a.harness = 'omp';
    else if (t === '--pi') a.harness = 'pi';
    else if (t === '--claude') a.harness = 'claude';
    else if (t === '--codex') a.harness = 'codex';
    else if (t === '--list' || t === '-l') a.list = true;
    else if (t === 'update' || t === '--update') a.update = true;
    else if (t === 'image' || t === 'image-gen' || t === '--image') a.image = true;
    else if (t === '--dry-run') a.dryRun = true;
    else if (t === '--safe') a.safe = true;
    else if (t === '--help' || t === '-h') a.help = true;
    else if (t === '--version' || t === '-v') a.version = true;
    else if ((t === 'account' || t === 'profile' || t === 'switch') && !a.provider) {
      a.provider = 'account';
      if (argv[i + 1] && !argv[i + 1].startsWith('-')) a.account = argv[++i];
    }
    else {
      // Unknown args belong to Claude. Once Claude args begin, preserve the
      // rest verbatim so values like `bro --resume update` are not re-parsed.
      a._.push(...argv.slice(i));
      break;
    }
  }
  return a;
}

// Quirky BRO CLI banner over the picker. Cyan fade, shades on.
const BANNER = [
  '\x1b[96m   ___  ___  ____     _______   ____\x1b[0m',
  '\x1b[96m  / _ )/ _ \\/ __ \\   / ___/ /  /  _/\x1b[0m',
  '\x1b[36m / _  / , _/ /_/ /  / /__/ /___/ /\x1b[0m',
  '\x1b[36m/____/_/|_|\\____/   \\___/____/___/\x1b[0m  (⌐■_■)',
  ''
].join('\n');

const tagOf = (p) =>
  p.mode === 'pool'
    ? 'rotate accounts'
    : p.mode === 'account'
      ? 'pick login'
    : p.mode === 'codex'
      ? 'chatgpt login'
    : p.mode === 'image'
      ? 'web ui'
      : p.mode === 'native'
        ? 'native'
        : p.mode === 'anthropic'
          ? 'anthropic-api'
          : 'via proxy';
const modelLabel = (m) => (m.name ? `${m.name}  ${m.id ? `\x1b[2m(${m.id})\x1b[0m` : ''}` : m.id || '(default)');
const normalizeHarness = (value) => {
  const h = String(value || 'claude').toLowerCase();
  if (h === 'claude' || h === 'claude-code') return 'claude';
  if (h === 'omp' || h === 'oh-my-pi') return 'omp';
  if (h === 'pi' || h === 'pi-coding-agent') return 'pi';
  if (h === 'codex' || h === 'codex-cli') return 'codex';
  return null;
};

// The [h] switch under both pickers rotates through the harnesses rather than
// flipping one on, so every option is visible before you commit to it.
const HARNESS_TOGGLE = (harness) => ({
  key: 'h',
  name: 'harness',
  label: 'Harness',
  value: harness,
  options: [
    { label: 'CLAUDE', value: 'claude' },
    { label: 'OMP', value: 'omp' },
    { label: 'PI', value: 'pi' },
    { label: 'CODEX', value: 'codex' }
  ],
  shortLabel: 'harness'
});

// `help` as a bare word (not just -h/--help), plus topic help, so a lost user
// typing `bro help`, `bro help image`, or `bro image help` lands somewhere
// useful instead of having the word passed through to the harness.
const isHelpWord = (a) => a === 'help' || a === '-h' || a === '--help';
const isImageWord = (a) => a === 'image' || a === 'image-gen' || a === '--image';

export async function main(argv) {
  if (['tokens', 'token-report'].includes(argv[0])) {
    return runTokenReport();
  }
  if (argv[0] === 'accounts') {
    return runPoolAccounts(argv.slice(1));
  }
  // `bro codex` on its own opens the Codex switcher, the way `bro account`
  // does for Claude; with a sub-command it manages logins and sessions, and
  // with a bare word it launches that profile. A flag is nobody's profile —
  // those keep falling through to the normal provider/harness path.
  if (argv[0] === 'codex' && (argv[1] == null || !argv[1].startsWith('-'))) {
    ensureDefaultConfig();
    const config = loadConfig();
    return runCodexCommand(argv.slice(1), {
      skipPermissions: !argv.includes('--safe') && config.dangerouslySkipPermissions !== false
    });
  }

  // Help dispatch: `bro help [topic]` and `bro image help|-h|--help`.
  if (argv[0] === 'help' || (isImageWord(argv[0]) && isHelpWord(argv[1]))) {
    if (isImageWord(argv[0]) || isImageWord(argv[1])) {
      ensureDefaultConfig();
      console.log(imageHelp(loadConfig()));
    } else {
      console.log(HELP);
    }
    return 0;
  }

  const args = parseArgs(argv);
  if (args.help) { console.log(HELP); return 0; }
  if (args.version) { console.log(pkg.version); return 0; }

  if (args.update) {
    try {
      const r = await updateModels();
      console.log(`Updated models from ${r.source}`);
      console.log(`  ${r.providers} providers · ${r.models} models`);
      console.log(`  stored at ${r.cache}`);
      return 0;
    } catch (e) {
      console.error(`Update failed: ${e.message}`);
      console.error('Kept the existing local copy.');
      return 1;
    }
  }

  ensureDefaultConfig();
  const config = loadConfig();
  // Flags win, then the harness used last time (the menu toggle is sticky),
  // then the configured default.
  let harness = normalizeHarness(args.harness || lastHarness() || config.defaultHarness || 'claude');
  if (!harness) {
    console.error(`Unknown harness: ${args.harness || config.defaultHarness}  (use: claude, omp, pi or codex)`);
    return 1;
  }

  // A command-line harness is already a completed selection. Persist it now,
  // even when setup or provider authentication later fails. Dry runs stay pure.
  if (!args.dryRun && !args.image && args.harness) rememberHarness(harness);

  // `bro image` goes straight to the image-gen web UI (no claude involved).
  if (args.image) {
    return runImageGen({ config, apiId: args.provider, dryRun: args.dryRun });
  }

  const data = await loadModels();
  // The account pool and image gen are always pinned on top — no models.json entry needed.
  const providers = [IMAGE_PROVIDER, POOL_PROVIDER, ACCOUNT_PROVIDER, CODEX_PROVIDER, ...mergeProviders(data, config.providers)];

  if (!providers.length) {
    console.error('No providers available. Check your network or ~/.bro/config.json.');
    return 1;
  }

  if (args.list) {
    for (const p of providers) {
      console.log(`\n${p.name || p.id}  \x1b[2m(${p.id} · ${tagOf(p)})\x1b[0m`);
      for (const m of p.models || []) console.log(`  - ${m.id || '(default)'}${m.name ? `  ${m.name}` : ''}`);
    }
    return 0;
  }

  // 1) provider + model in one two-column picker: the left column lists
  // providers, the right column live-previews the highlighted provider's
  // models (OpenRouter's live catalogue loads while you browse; Image Gen
  // shows its image APIs). Enter takes the highlighted model; →/← moves
  // between the columns.
  let provider;
  let picked = null; // combined-picker result (null when -p skipped the menu)
  let skip = !args.safe && config.dangerouslySkipPermissions !== false;
  if (args.provider) {
    provider = providers.find(
      (p) => p.id === args.provider || (p.name || '').toLowerCase() === args.provider.toLowerCase()
    );
    if (!provider) { console.error(`Unknown provider: ${args.provider}  (try: bro --list)`); return 1; }
  } else {
    const modelChildren = (models) => (models || []).map((m) => ({ label: modelLabel(m), value: m.id ?? '' }));
    const childrenFor = (p) => {
      if (p.mode === 'image') {
        return mergeImageApis(config.imageApis).map((a) => ({
          label: `${a.name || a.id}  \x1b[2m${a.models?.[0]?.id || ''}\x1b[0m`,
          value: a.id
        }));
      }
      if (p.id === 'openrouter') return async () => modelChildren((await loadOpenRouterModels()) || p.models);
      // Account profiles with live usage stats (5h/week/Fable) in the right
      // pane, followed by the sessions those profiles can resume.
      if (p.mode === 'account') return accountProfileChoices;
      // Codex logins with the sessions they can resume — the same shape as the
      // account pane. (Its models come from the subscription and are chosen
      // after the login, not here.)
      if (p.mode === 'codex') return codexProfileChoices;
      return (p.models || []).length ? modelChildren(p.models) : null;
    };
    // Providers that are ready to launch (key saved / env var / no key needed)
    // go on top in green, the rest below a divider.
    const hasKey = (id, keyEnv) => Boolean(config.keys?.[id] || (keyEnv && process.env[keyEnv]));
    const isConfigured = (p) => {
      if (p.mode === 'image') return mergeImageApis(config.imageApis).some((a) => hasKey(a.id, a.keyEnv));
      if (p.mode === 'native' || p.noKey || ['pool', 'account', 'codex'].includes(p.mode)) return true;
      return hasKey(p.id, p.keyEnv);
    };
    const toChoice = (p, configured) => ({
      label: p.name || p.id,
      value: p,
      color: configured ? '\x1b[32m' : '',
      detail: tagOf(p),
      children: childrenFor(p),
      filterableChildren: p.id === 'openrouter' || p.mode === 'account' || p.mode === 'codex',
      // Codex's pane lists logins rather than models, so it reopens on the
      // login used last.
      childValue: p.mode === 'codex' ? lastProfileFor(p.id) : lastModelFor(p.id)
    });
    const ready = providers.filter((p) => isConfigured(p));
    const rest = providers.filter((p) => !isConfigured(p));
    const choices = [
      ...ready.map((p) => toChoice(p, true)),
      ...(ready.length && rest.length ? [{ divider: true }] : []),
      ...rest.map((p) => toChoice(p, false))
    ];

    const lastP = lastProvider();
    const choice = await selectColumns({
      message: 'Choose a provider and model:',
      startIndex: Math.max(0, choices.findIndex((c) => c.value?.id === lastP)),
      choices,
      clearScreen: true,
      banner: BANNER,
      toggle: { label: 'Skip permissions', value: skip },
      toggles: [HARNESS_TOGGLE(harness)]
    }).catch(() => null);
    if (!choice) { console.log('Cancelled.'); return 0; }
    provider = choice.value;
    picked = choice;
    if (choice.toggleOn !== undefined) skip = choice.toggleOn;
    if (choice.toggles?.harness) harness = choice.toggles.harness;
    if (!args.dryRun) rememberHarness(harness);
  }

  // Image gen: the picker's right column already chose the image API (falls
  // back to imagegen's own menu when it didn't), then serve the local web UI.
  // Deliberately not remembered as the default provider — it's the exception.
  if (provider.mode === 'image') {
    return runImageGen({ config, apiId: picked?.child?.value, dryRun: args.dryRun });
  }

  // Codex: with the claude/omp harness this ensures the ChatGPT subscription
  // login, fetches its live model list, picks one and bridges the harness to
  // it. With the codex harness — or a session picked from the right column,
  // which only the codex CLI can read — it runs codex itself instead.
  if (provider.mode === 'codex') {
    // The right column mixes logins (a string name, '' for this machine's),
    // the manage entry and resumable sessions (objects).
    const child = picked?.child?.value;
    const session = child?.kind === 'codex-session' ? child : null;
    const result = await runCodex({
      model: args.model,
      harness,
      profile: args.account || (typeof child === 'string' ? child : ''),
      session,
      // A session picked here came from a list spanning every login, so the
      // launcher asks which one should resume it.
      chooseProfile: Boolean(session),
      manage: child?.manage === true,
      extraArgs: args._,
      skipPermissions: !args.safe && config.dangerouslySkipPermissions !== false,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 2) model — usually already picked in the combined menu above. With the
  // omp harness the model is left to omp itself (bro only writes the provider
  // into omp's models.yml), unless one was explicitly picked in the model
  // column or forced with -m.
  // (The account provider's picker children are profiles, not models.)
  let model = args.model;
  if (model == null && picked?.child && provider.mode !== 'account' && (harness !== 'omp' || picked.childFocused)) {
    model = picked.child.value;
  }

  // OpenRouter: swap in its complete live catalogue when the model still has
  // to be chosen here (-p path) or omp needs the current model list. On fetch
  // failure the cached copy is used; failing that, the static list from
  // models.json stays.
  if (provider.id === 'openrouter' && !args.dryRun && (model == null || harness === 'omp')) {
    if (isInteractive) process.stdout.write('\x1b[2mFetching OpenRouter models…\x1b[0m\r');
    const live = await loadOpenRouterModels();
    if (isInteractive) process.stdout.write('\x1b[2K');
    if (live) provider.models = live;
  }

  const models = provider.models || [];
  if (model == null) {
    if (harness === 'omp' || !models.length) {
      model = '';
    } else {
      const lastM = lastModelFor(provider.id);
      const choice = await select({
        message: `Choose a model for ${provider.name || provider.id}:`,
        startIndex: lastM != null ? Math.max(0, models.findIndex((m) => (m.id ?? '') === lastM)) : 0,
        choices: models.map((m) => ({ label: modelLabel(m), value: m.id ?? '' })),
        filterable: provider.id === 'openrouter',
        toggle: { label: 'Skip permissions', value: skip },
        toggles: [HARNESS_TOGGLE(harness)]
      }).catch(() => null);
      if (choice == null) { console.log('Cancelled.'); return 0; }
      model = choice.value;
      if (choice.toggleOn !== undefined) skip = choice.toggleOn;
      if (choice.toggles?.harness) harness = choice.toggles.harness;
      if (!args.dryRun) rememberHarness(harness);
    }
  }

  // Account pool: its own setup → start proxy → launch the selected harness
  // against the local Anthropic-compatible pool endpoint.
  if (provider.mode === 'pool') {
    if (!args.dryRun) rememberSelection(provider.id, model, harness);
    const result = await runPool({
      model,
      extraArgs: args._,
      skipPermissions: skip,
      harness,
      dryRun: args.dryRun
    });
    // A dry run normally describes what would happen; a refused combination
    // has already said why and only has its exit code left to report.
    if (args.dryRun && typeof result !== 'number') { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // Account profile: launch standard Claude Code using one isolated logged-in
  // account directory. This is a direct login switch, not the multi-account pool.
  if (provider.mode === 'account') {
    // The right column mixes profiles (a string name) with resumable sessions
    // (an object). Sessions carry their source owner; the launcher asks which
    // profile should resume them.
    const child = picked?.child?.value;
    const session = child && typeof child === 'object' && child.kind === 'session' ? child : null;
    // A selected session deliberately leaves the destination account open:
    // runAccountProfile asks which login should resume it, preselecting owner.
    const accountName = args.account || (session ? '' : (typeof child === 'string' ? child : ''));
    // Remember the account (not a model) so the picker preselects it next time.
    if (!args.dryRun && !session) rememberSelection(provider.id, accountName);
    const result = await runAccountProfile({
      accountName,
      model,
      session,
      extraArgs: args._,
      skipPermissions: skip,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 3) key (skipped for native Claude and noKey/local providers)
  let apiKey = '';
  if (provider.mode !== 'native' && !provider.noKey) {
    apiKey =
      (config.keys && config.keys[provider.id]) ||
      (provider.keyEnv && process.env[provider.keyEnv]) ||
      '';
    if (!apiKey && !args.dryRun) {
      const hint = provider.keyUrl ? `  \x1b[2m(get one: ${provider.keyUrl})\x1b[0m` : '';
      apiKey = await promptHidden(`Enter API key for ${provider.name || provider.id}${hint}\n> `).catch(() => '');
      if (!apiKey) { console.error('No key entered.'); return 1; }
      setKey(provider.id, apiKey);
      console.log(`Saved to ${CONFIG_PATH}`);
    }
  }

  if (!args.dryRun) rememberSelection(provider.id, model, harness);

  const result = await launch({
    provider,
    model,
    apiKey,
    extraArgs: args._,
    skipPermissions: skip,
    harness,
    dryRun: args.dryRun
  });

  if (args.dryRun) {
    console.log(JSON.stringify(result, null, 2));
    return 0;
  }
  return typeof result === 'number' ? result : 0;
}
