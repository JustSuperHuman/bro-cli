import { readFileSync } from 'node:fs';
import { loadConfig, ensureDefaultConfig, setKey, CONFIG_PATH } from './config.js';
import { loadModels, mergeProviders, updateModels, REMOTE_URL } from './models.js';
import { select, promptHidden } from './ui.js';
import { launch } from './launch.js';
import { runPool, runPoolAccounts, runAccountProfile, POOL_PROVIDER, ACCOUNT_PROVIDER } from './pool.js';
import { runImageGen, IMAGE_PROVIDER } from './imagegen.js';
import { runCodex, runCodexCommand, CODEX_PROVIDER } from './codex.js';
import { rememberSelection, lastProvider, lastModelFor, lastHarness } from './state.js';

const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

const HELP = `bro — run Claude Code against any provider/model.

Usage:
  bro                    Pick a provider, then a model (interactive)
  bro -p pool            Multiple Claude Account Proxy — pool many Claude
                         plans, then launch Claude Code across them
  bro account [name]     Pick/run one logged-in Claude account profile
  bro accounts list      List pool accounts
  bro accounts login <name>
                         Add/log in a Claude account for the pool
  bro accounts import <name>
                         Copy this machine's current Claude login into the pool
  bro image              Image generation — pick an API, then a self-hosted
                         web UI opens (images save to ./.bro/image-gen)
  bro image -p <api>     Skip the image API menu (e.g. bro image -p yunwu)
  bro -p codex           Run Claude Code on your ChatGPT subscription — logs
                         in, fetches the live model list, and bridges through a
                         local Anthropic-compatible server (no codex CLI needed)
  bro codex login        Log in to (or switch) your ChatGPT subscription
  bro codex status       Show ChatGPT subscription login status
  bro codex logout       Remove stored ChatGPT credentials
  bro -p <provider>      Skip the provider menu (id or name)
  bro --account <name>   Launch Claude with a logged-in account profile
  bro -m <model>         Skip the model menu (use with -p)
  bro --harness <name>   Choose harness: claude (default) or omp
  bro --omp              Launch with omp instead of Claude Code; bro sets up
                         the provider and omp picks the model (-m overrides)
  bro -l, --list         List every provider and model
  bro update             Refresh the model list from GitHub and cache it
  bro --dry-run          Show what would run; launch nothing
  bro --safe             Don't pass --dangerously-skip-permissions
  bro -h, --help         Show this help
  bro -v, --version      Show version
  bro --resume <id>      Pick provider/model, then pass args to the harness
  bro -- <args...>       Force everything after -- straight to the harness

Put bro flags first. The first unrecognized arg, and everything after it,
is passed verbatim to the selected harness after provider/model selection.

Config:  ${CONFIG_PATH}
Models:  ${REMOTE_URL}
Docs:    https://justgains.com`;

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const t = argv[i];
    if (t === '--') { a._.push(...argv.slice(i + 1)); break; }
    if (t === '--provider' || t === '-p') a.provider = argv[++i];
    else if (t === '--account') { a.provider = 'account'; a.account = argv[++i]; }
    else if (t === '--model' || t === '-m') a.model = argv[++i];
    else if (t === '--harness') a.harness = argv[++i];
    else if (t === '--omp') a.harness = 'omp';
    else if (t === '--claude') a.harness = 'claude';
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
  return null;
};

export async function main(argv) {
  if (argv[0] === 'accounts') {
    return runPoolAccounts(argv.slice(1));
  }
  if (argv[0] === 'codex' && ['login', 'logout', 'status'].includes(argv[1])) {
    return runCodexCommand(argv.slice(1));
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
    console.error(`Unknown harness: ${args.harness || config.defaultHarness}  (use: claude or omp)`);
    return 1;
  }

  // `bro image` goes straight to the image-gen web UI (no claude involved).
  if (args.image) {
    return runImageGen({ config, apiId: args.provider, dryRun: args.dryRun });
  }

  const data = await loadModels();
  // The account pool and image gen are always pinned on top — no models.json entry needed.
  const providers = [POOL_PROVIDER, ACCOUNT_PROVIDER, CODEX_PROVIDER, IMAGE_PROVIDER, ...mergeProviders(data, config.providers)];

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

  // 1) provider
  let provider;
  if (args.provider) {
    provider = providers.find(
      (p) => p.id === args.provider || (p.name || '').toLowerCase() === args.provider.toLowerCase()
    );
    if (!provider) { console.error(`Unknown provider: ${args.provider}  (try: bro --list)`); return 1; }
  } else {
    const width = Math.max(...providers.map((p) => (p.name || p.id).length));
    const lastP = lastProvider();
    const choice = await select({
      message: 'Choose a provider:',
      startIndex: Math.max(0, providers.findIndex((p) => p.id === lastP)),
      choices: providers.map((p) => ({
        label: `${(p.name || p.id).padEnd(width)}  \x1b[2m${tagOf(p)}\x1b[0m`,
        value: p
      })),
      toggles: [{
        key: 'h',
        name: 'ompHarness',
        label: 'Harness',
        value: harness === 'omp',
        onLabel: 'OMP',
        offLabel: 'CLAUDE',
        shortLabel: 'harness'
      }]
    }).catch(() => null);
    if (!choice) { console.log('Cancelled.'); return 0; }
    provider = choice.value;
    if (choice.toggles?.ompHarness !== undefined) harness = choice.toggles.ompHarness ? 'omp' : 'claude';
  }

  // Image gen: pick an image API, then serve the local web UI.
  if (provider.mode === 'image') {
    if (!args.dryRun) rememberSelection(provider.id, lastModelFor(provider.id) ?? '');
    return runImageGen({ config, dryRun: args.dryRun });
  }

  // Codex: ensure the ChatGPT subscription login, fetch its live model list,
  // pick one, and launch the codex CLI (its own harness — claude/omp not used).
  if (provider.mode === 'codex') {
    const result = await runCodex({
      model: args.model,
      harness,
      extraArgs: args._,
      skipPermissions: !args.safe && config.dangerouslySkipPermissions !== false,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 2) model (+ an easy skip-permissions toggle — Tab to flip)
  // With the omp harness the model menu is skipped: bro only writes the
  // provider (and its model list) into omp's models.yml and lets omp pick the
  // model itself. An explicit -m still forces one via --model.
  let model = args.model;
  let skip = !args.safe && config.dangerouslySkipPermissions !== false;
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
        toggle: { label: 'Skip permissions', value: skip },
        toggles: [{
          key: 'h',
          name: 'ompHarness',
          label: 'Harness',
          value: harness === 'omp',
          onLabel: 'OMP',
          offLabel: 'CLAUDE',
          shortLabel: 'harness'
        }]
      }).catch(() => null);
      if (choice == null) { console.log('Cancelled.'); return 0; }
      model = choice.value;
      if (choice.toggleOn !== undefined) skip = choice.toggleOn;
      if (choice.toggles?.ompHarness !== undefined) harness = choice.toggles.ompHarness ? 'omp' : 'claude';
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
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // Account profile: launch standard Claude Code using one isolated logged-in
  // account directory. This is a direct login switch, not the multi-account pool.
  if (provider.mode === 'account') {
    if (!args.dryRun) rememberSelection(provider.id, model, 'claude');
    const result = await runAccountProfile({
      accountName: args.account,
      model,
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
