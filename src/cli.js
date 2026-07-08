import { readFileSync } from 'node:fs';
import { loadConfig, ensureDefaultConfig, setKey, configPermissionMode, CONFIG_PATH } from './config.js';
import { loadModels, mergeProviders, updateModels, REMOTE_URL } from './models.js';
import { select, promptHidden } from './ui.js';
import { launch } from './launch.js';
import { runPool, runPoolAccounts, POOL_PROVIDER } from './pool.js';
import { runImageGen, IMAGE_PROVIDER } from './imagegen.js';
import { rememberSelection, lastProvider, lastModelFor } from './state.js';

const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

const HELP = `bro — run Claude Code against any provider/model.

Usage:
  bro                    Pick a provider, then a model (interactive)
  bro -p pool            Multiple Claude Account Proxy — pool many Claude
                         plans, then launch Claude Code across them
  bro accounts list      List pool accounts
  bro accounts login <name>
                         Add/log in a Claude account for the pool
  bro accounts import <name>
                         Copy this machine's current Claude login into the pool
  bro image              Image generation — pick an API, then a self-hosted
                         web UI opens (images save to ./.bro/image-gen)
  bro image -p <api>     Skip the image API menu (e.g. bro image -p yunwu)
  bro -p <provider>      Skip the provider menu (id or name)
  bro -m <model>         Skip the model menu (use with -p)
  bro -l, --list         List every provider and model
  bro update             Refresh the model list from GitHub and cache it
  bro --dry-run          Show what would run; launch nothing
  bro --safe             Start Claude in manual mode (prompt for everything)
                         instead of the default auto mode
  bro -h, --help         Show this help
  bro -v, --version      Show version
  bro --resume <id>      Pick provider/model, then pass args to claude
  bro -- <args...>       Force everything after -- straight to claude

Put bro flags first. The first unrecognized arg, and everything after it,
is passed verbatim to claude after provider/model selection.

Config:  ${CONFIG_PATH}
Models:  ${REMOTE_URL}
Docs:    https://justgains.com`;

function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const t = argv[i];
    if (t === '--') { a._.push(...argv.slice(i + 1)); break; }
    if (t === '--provider' || t === '-p') a.provider = argv[++i];
    else if (t === '--model' || t === '-m') a.model = argv[++i];
    else if (t === '--list' || t === '-l') a.list = true;
    else if (t === 'update' || t === '--update') a.update = true;
    else if (t === 'image' || t === 'image-gen' || t === '--image') a.image = true;
    else if (t === '--dry-run') a.dryRun = true;
    else if (t === '--safe') a.safe = true;
    else if (t === '--help' || t === '-h') a.help = true;
    else if (t === '--version' || t === '-v') a.version = true;
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
    : p.mode === 'image'
      ? 'web ui'
      : p.mode === 'native'
        ? 'native'
        : p.mode === 'anthropic'
          ? 'anthropic-api'
          : 'via proxy';
const modelLabel = (m) => (m.name ? `${m.name}  ${m.id ? `\x1b[2m(${m.id})\x1b[0m` : ''}` : m.id || '(default)');

export async function main(argv) {
  if (argv[0] === 'accounts') {
    return runPoolAccounts(argv.slice(1));
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

  // `bro image` goes straight to the image-gen web UI (no claude involved).
  if (args.image) {
    return runImageGen({ config, apiId: args.provider, dryRun: args.dryRun });
  }

  const data = await loadModels();
  // The account pool and image gen are always pinned on top — no models.json entry needed.
  const providers = [POOL_PROVIDER, IMAGE_PROVIDER, ...mergeProviders(data, config.providers)];

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
      }))
    }).catch(() => null);
    if (!choice) { console.log('Cancelled.'); return 0; }
    provider = choice.value;
  }

  // Image gen: pick an image API, then serve the local web UI.
  if (provider.mode === 'image') {
    if (!args.dryRun) rememberSelection(provider.id, lastModelFor(provider.id) ?? '');
    return runImageGen({ config, dryRun: args.dryRun });
  }

  // Account pool: its own setup → start proxy → launch claude flow.
  if (provider.mode === 'pool') {
    const poolMode = args.safe ? 'manual' : configPermissionMode(config);
    if (!args.dryRun) rememberSelection(provider.id, '');
    const result = await runPool({
      extraArgs: args._,
      permissionMode: poolMode,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 2) model. Claude starts in auto mode by default; --safe forces manual.
  //    The "Skip permissions" toggle (Tab to flip) opts into the dangerous
  //    --dangerously-skip-permissions bypass for this launch.
  let model = args.model;
  //    'auto' | 'manual' | 'bypass'
  let mode = args.safe ? 'manual' : configPermissionMode(config);
  const models = provider.models || [];
  if (model == null) {
    if (!models.length) {
      model = '';
    } else {
      const lastM = lastModelFor(provider.id);
      const choice = await select({
        message: `Choose a model for ${provider.name || provider.id}:`,
        startIndex: lastM != null ? Math.max(0, models.findIndex((m) => (m.id ?? '') === lastM)) : 0,
        choices: models.map((m) => ({ label: modelLabel(m), value: m.id ?? '' })),
        toggle: { label: 'Skip permissions', value: mode === 'bypass' }
      }).catch(() => null);
      if (choice == null) { console.log('Cancelled.'); return 0; }
      model = choice.value;
      // Turning the toggle on means bypass; turning it off drops back to the
      // non-bypass posture (manual under --safe, otherwise auto).
      if (choice.toggleOn !== undefined) {
        mode = choice.toggleOn ? 'bypass' : args.safe ? 'manual' : 'auto';
      }
    }
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

  if (!args.dryRun) rememberSelection(provider.id, model);

  const result = await launch({
    provider,
    model,
    apiKey,
    extraArgs: args._,
    permissionMode: mode,
    dryRun: args.dryRun
  });

  if (args.dryRun) {
    console.log(JSON.stringify(result, null, 2));
    return 0;
  }
  return typeof result === 'number' ? result : 0;
}
