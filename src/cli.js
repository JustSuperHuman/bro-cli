import { readFileSync } from 'node:fs';
import { loadConfig, ensureDefaultConfig, setKey, CONFIG_PATH } from './config.js';
import { loadModels, mergeProviders, updateModels, REMOTE_URL } from './models.js';
import { select, promptHidden } from './ui.js';
import { launch } from './launch.js';
import { runPool, runPoolAccounts, POOL_PROVIDER } from './pool.js';
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
  bro -p <provider>      Skip the provider menu (id or name)
  bro -m <model>         Skip the model menu (use with -p)
  bro -l, --list         List every provider and model
  bro update             Refresh the model list from GitHub and cache it
  bro --dry-run          Show what would run; launch nothing
  bro --safe             Don't pass --dangerously-skip-permissions
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
  const data = await loadModels();
  // The account pool is always the top option — no models.json entry needed.
  const providers = [POOL_PROVIDER, ...mergeProviders(data, config.providers)];

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

  // Account pool: its own setup → start proxy → launch claude flow.
  if (provider.mode === 'pool') {
    const skipPool = !args.safe && config.dangerouslySkipPermissions !== false;
    if (!args.dryRun) rememberSelection(provider.id, '');
    const result = await runPool({
      extraArgs: args._,
      skipPermissions: skipPool,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 2) model (+ an easy skip-permissions toggle — Tab to flip)
  let model = args.model;
  let skip = !args.safe && config.dangerouslySkipPermissions !== false;
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
        toggle: { label: 'Skip permissions', value: skip }
      }).catch(() => null);
      if (choice == null) { console.log('Cancelled.'); return 0; }
      model = choice.value;
      if (choice.toggleOn !== undefined) skip = choice.toggleOn;
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
    skipPermissions: skip,
    dryRun: args.dryRun
  });

  if (args.dryRun) {
    console.log(JSON.stringify(result, null, 2));
    return 0;
  }
  return typeof result === 'number' ? result : 0;
}
