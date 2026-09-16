import { readFileSync } from 'node:fs';
import { loadConfig, ensureDefaultConfig, setKey, CONFIG_PATH } from './config.js';
import {
  loadModels,
  loadOpenRouterModels,
  readOpenRouterCache,
  attachStats,
  refreshOpenRouterStats,
  mergeProviders,
  updateModels,
  REMOTE_URL
} from './models.js';
import { modelRow, modelHeader, topQuality, anySpeed, catalogueIndex, enrichFromCatalogue } from './model-info.js';
import {
  isNewApi,
  loadNewApiCatalogue,
  readNewApiCache,
  modelById,
  withTier,
  providerForModel,
  newApiKey,
  tierKeyId,
  tierRow,
  tierHeader,
  priceLabel
} from './newapi.js';
import { select, selectColumns, promptHidden, isInteractive } from './ui.js';
import { launch } from './launch.js';
import { runPool, runPoolAccounts, runAccountProfile, accountProfileChoices, POOL_PROVIDER, ACCOUNT_PROVIDER } from './pool.js';
import {
  runJustImagine,
  runImagineCommand,
  runServiceCommand,
  imagineHelp,
  mergeImageApis,
  parseImagineArgs,
  IMAGINE_PROVIDER
} from './justimagine.js';
import { runCodex, runCodexCommand, codexProfileChoices, CODEX_PROVIDER } from './codex.js';
import { runTokenReport } from './token-report.js';
import { runProfilesReport } from './profile-report.js';
import { ensureHarnessTool, HARNESS_INSTALLS, updateHarnessTool } from './proc.js';
import { ensureDshProfilesPlugin } from './dsh-profile-plugin.js';
import { rememberSelection, rememberHarness, rememberTier, lastProvider, lastModelFor, lastProfileFor, lastTierFor, lastHarness } from './state.js';
import { note } from './out.js';
import {
  browsersWithClaudeExtension,
  browsersWithMcpChromeExtension,
  browserBackend,
  claudeBrowserEnabled,
  preferredBrowser,
  resolveBrowser,
  runClaudeBrowserCommand,
  setClaudeBrowserEnabled,
  setPreferredBrowser
} from './claude-browser.js';

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
  bro browser setup [browser]
                         Give every model bro launches — Claude, GLM, the
                         account pool — your own browser, shared by all of
                         them at once (default: Edge)
  bro browser setup edge --mcp-chrome
                         Use the local mcp-chrome extension bridge at
                         http://127.0.0.1:12306/mcp (the default)
  bro browser setup --dedicated
                         Use a separate persistent Edge window instead
  bro browser use <edge|chrome|brave|vivaldi|chromium|opera|auto>
                         Pin which browser sessions drive, and remember it
  bro browser owner <login|auto>
                         Which Claude login the extension is signed in as —
                         every other login shares its browser through it
  bro browser open       Open the shared Claude browser
  bro browser test       Name every connected browser and say which one wins
  bro browser reconnect  Put the extension back on the bridge when sessions
                         report no browser (sessions do this themselves too)
  bro browser verify     Check every Claude login reaches the browser, each
                         through the route its own sessions use
  bro browser status     Show browser backend and live connection readiness
  bro browser clean      Delete unused bro browser data from earlier setups
  bro browser disable    Stop connecting sessions to the browser
  bro imagine            JustImagine — generate images and video in a
                         self-hosted gallery (folders live in ./.bro/justimagine)
  bro imagine -p <api>   Skip the API menu (e.g. bro imagine -p openrouter)
  bro imagine service install
                         Serve this directory's gallery in the background,
                         from every login on (no admin needed) — run it
                         elevated and it starts with the machine instead
  bro imagine service status | start | stop | logs | uninstall
  bro imagine open       Open the running JustImagine service
  bro imagine skill      Install the generate-images-videos skill, which
                         teaches an agent to batch-generate through its API
  bro imagine help       JustImagine help (APIs, models, folders, paths)
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
  bro profiles           24h / 7d / 30d usage for all Claude + Codex profiles
  bro tokens             Lifetime + last-30d tokens for all Claude + Codex
                         profiles
  bro --print "<prompt>" Headless: answer once on stdout and exit. The model's
                         answer is all that reaches stdout, so it pipes.
                         Omit the prompt to read it from stdin. Add Claude
                         Code's own flags after it, e.g.
                           bro -p zai -m glm-5.3 --print "hi" --output-format json
  bro -p <provider>      Skip the provider menu (id or name)
  bro --account <name>   Launch with a logged-in profile (Claude, or the Codex
                         profile of that name with -p codex)
  bro -m <model>         Skip the model menu (use with -p)
  bro --tier <group>     Pick the upstream route at a new-api relay (OpenLux,
                         Yunwu). One model is served by several, each with its
                         own price — gpt-6-astra is $10/M through Openai-Gpt-2
                         and $0.37/M through Codex-Gpt-1, e.g.
                           bro -p openlux -m gpt-6-astra --tier Codex-Gpt-1
  bro --harness <name>   Choose harness: claude (default), omp, pi, codex or dsh
  bro --omp              Launch with omp instead of Claude Code; bro sets up
                         the provider and omp picks the model (-m overrides)
  bro --pi               Launch with Pi; bro sets up the provider/model and
                         passes API credentials only through the environment
  bro --codex            Launch the codex CLI instead of Claude Code (your
                         ChatGPT login, or a provider serving OpenAI's
                         Responses API)
  bro --dsh              Launch DeepSeek Harness Web; bro syncs every model
                         provider and autoloads the Claude/Codex profile switcher
  bro harness install dsh
                         Install a harness explicitly (first launch also does)
  bro harness update dsh Update DeepSeek Harness to the current npm latest tag
  bro -l, --list         List every provider and model
  bro update             Refresh the model list from GitHub, the OpenRouter
                         catalogue (ages, prices, benchmark scores) and every
                         model's speed measurement
  bro --dry-run          Show what would run; launch nothing
  bro --safe             Don't pass --dangerously-skip-permissions
  bro help, -h, --help   Show this help (bro help imagine for JustImagine)
  bro -v, --version      Show version
  bro --resume <id>      Pick provider/model, then pass args to the harness
  bro -- <args...>       Force everything after -- straight to the harness

Put bro flags first. The first unrecognized arg, and everything after it,
is passed verbatim to the selected harness after provider/model selection.
Missing harnesses are installed automatically on first use.

bro's own progress goes to stderr, so stdout is always just the harness's
output. A headless run never opens a menu, never asks for a key, and leaves
your remembered provider/model/harness alone.

Config:  ${CONFIG_PATH}
Models:  ${REMOTE_URL}
Docs:    https://justgains.com`;

export function parseArgs(argv) {
  const a = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const t = argv[i];
    if (t === '--') { a._.push(...argv.slice(i + 1)); break; }
    if (t === '--provider' || t === '-p') a.provider = argv[++i];
    // bro's -p was taken by --provider long before Claude Code's -p/--print
    // meant "answer once and exit". --print is bro's spelling of the latter,
    // so a headless run needs no `--` fencing. The prompt is optional: with
    // none, the harness reads it from stdin the way it always has.
    else if (t === '--print') {
      a.print = argv[i + 1] != null && !argv[i + 1].startsWith('-') ? argv[++i] : '';
    }
    // --print=… so a prompt may itself begin with a dash, which the bare form
    // would otherwise read as the next flag.
    else if (t.startsWith('--print=')) a.print = t.slice('--print='.length);
    // --account names a login profile. On its own that means the Claude
    // account switcher; alongside -p it just names the profile (Codex has
    // them too), so an explicit provider is never overridden.
    else if (t === '--account') { a.account = argv[++i]; a.provider = a.provider || 'account'; }
    else if (t === '--model' || t === '-m') a.model = argv[++i];
    // --tier names the upstream route at a new-api relay (OpenLux, Yunwu):
    // the same model is served by several, each with its own price.
    else if (t === '--tier' || t === '--group') a.tier = argv[++i];
    else if (t === '--harness') a.harness = argv[++i];
    else if (t === '--omp') a.harness = 'omp';
    else if (t === '--pi') a.harness = 'pi';
    else if (t === '--claude') a.harness = 'claude';
    else if (t === '--codex') a.harness = 'codex';
    else if (t === '--dsh' || t === '--deepseek') a.harness = 'dsh';
    else if (t === '--list' || t === '-l') a.list = true;
    else if (t === 'update' || t === '--update') a.update = true;
    else if (isImagineWord(t)) a.imagine = true;
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
  p.catalogue === 'newapi'
    ? 'relay · pick a tier'
  : p.mode === 'pool'
    ? 'rotate accounts'
    : p.mode === 'account'
      ? 'pick login'
    : p.mode === 'codex'
      ? 'chatgpt login'
    : p.mode === 'imagine'
      ? 'images + video'
      : p.mode === 'native'
        ? 'native'
        : p.mode === 'anthropic'
          ? 'anthropic-api'
          : 'via proxy';
// A provider's models as picker rows: a column-heading row, then one row per
// model laid out for whatever width the column gets (age · cost · speed ·
// quality, see model-info.js). Cost is hidden where the price is not what the
// user pays — a subscription login or a local model. The speed column only
// appears once some row has a measurement.
const showCostFor = (p) => !(p.mode === 'native' || p.noKey);
function modelRows(models, provider) {
  const list = models || [];
  const now = Date.now();
  const top = topQuality(list);
  const opts = { now, top, showCost: showCostFor(provider), showSpeed: anySpeed(list) };
  return [
    { divider: true, header: true, label: (width) => modelHeader({ width, ...opts }) },
    ...list.map((m) => ({
      label: (width) => modelRow(m, { width, ...opts }),
      value: m.id ?? '',
      filterText: `${m.name || ''} ${m.id || ''}`
    }))
  ];
}
const modelHeaderFor = (models, provider) => (width) =>
  modelHeader({ width, showCost: showCostFor(provider), showSpeed: anySpeed(models) });

// The OpenRouter key also unlocks per-model speed stats (OpenRouter reports
// throughput only to authenticated callers).
const openRouterKey = (config) => config.keys?.openrouter || process.env.OPENROUTER_API_KEY || '';

// A catalogue copy up to this old is shown as-is; the picker refreshes it in
// the background rather than making the user wait.
const CATALOGUE_FRESH_MS = 6 * 60 * 60 * 1000;

// Other providers' models are looked up in the OpenRouter catalogue by model
// id, so "glm-5.3" at Z.ai still shows its age and benchmark score.
const enrichModels = (models, catalogue) => {
  if (!catalogue?.length) return attachStats(models || []);
  const index = catalogueIndex(catalogue);
  return attachStats((models || []).map((m) => enrichFromCatalogue(m, index)));
};
const normalizeHarness = (value) => {
  const h = String(value || 'claude').toLowerCase();
  if (h === 'claude' || h === 'claude-code') return 'claude';
  if (h === 'omp' || h === 'oh-my-pi') return 'omp';
  if (h === 'pi' || h === 'pi-coding-agent') return 'pi';
  if (h === 'codex' || h === 'codex-cli') return 'codex';
  if (h === 'dsh' || h === 'deepseek' || h === 'deepseek-harness') return 'dsh';
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
    { label: 'CODEX', value: 'codex' },
    { label: 'DEEPSEEK', value: 'dsh' }
  ],
  shortLabel: 'harness'
});

// The [b] switch picks which browser the session's browser tools drive, and
// doubles as the on/off for them: OFF → AUTO → each browser carrying the
// Claude extension. AUTO is right whenever only one browser has it, which is
// why it sits first — naming one only matters when several are connected and
// the bridge would otherwise have to ask.
export function browserToggle({
  installed = browserBackend() === 'mcp-chrome' ? browsersWithMcpChromeExtension() : browsersWithClaudeExtension(),
  enabled = claudeBrowserEnabled(),
  chosen = preferredBrowser()
} = {}) {
  if (process.platform !== 'win32' || !installed.length) return null;
  const shortLabelFor = (name) => (resolveBrowser(name)?.id || name).toUpperCase();
  const options = [
    { label: 'OFF', value: 'off' },
    { label: 'AUTO', value: 'auto' },
    ...installed.map((name) => ({ label: shortLabelFor(name), value: name }))
  ];
  const value = !enabled ? 'off' : (chosen && installed.includes(chosen) ? chosen : 'auto');
  return { key: 'b', name: 'browser', label: 'Browser', value, options, shortLabel: 'browser' };
}

export function applyBrowserToggle(value) {
  if (value == null) return;
  if (value === 'off') {
    setClaudeBrowserEnabled(false);
    return;
  }
  setClaudeBrowserEnabled(true);
  setPreferredBrowser(value === 'auto' ? '' : value);
}

// A run is headless when nothing is going to be typed at it: the harness was
// asked to print one answer and exit, or bro has no terminal on both ends
// (piped, redirected, cron, CI). Headless runs must never stop to ask a
// question they cannot show, and must not put a browser window on screen.
const PRINT_FLAGS = new Set(['-p', '--print']);
export const isHeadlessRun = ({ print, harnessArgs = [], interactive = isInteractive }) =>
  print !== undefined || harnessArgs.some((arg) => PRINT_FLAGS.has(arg)) || !interactive;

// The one thing a headless run cannot do is open a menu. Say what to name
// instead, rather than reporting a cancellation nobody made.
function explainNoMenu(what) {
  console.error(`bro needs ${what} up front here — there is no terminal to show its menu on.`);
  console.error('  Name them on the command line:');
  console.error('    bro -p zai -m glm-5.3 --print "your prompt"');
  console.error('  bro --list prints every provider id and model id.');
}

// `help` as a bare word (not just -h/--help), plus topic help, so a lost user
// typing `bro help`, `bro help imagine`, or `bro imagine help` lands somewhere
// useful instead of having the word passed through to the harness.
const isHelpWord = (a) => a === 'help' || a === '-h' || a === '--help';
const isImagineWord = (a) => ['imagine', 'justimagine', 'image', 'image-gen', '--image', '--imagine'].includes(a);

async function runHarnessCommand(args) {
  const action = args[0];
  const rawName = args[1];
  const harness = normalizeHarness(rawName);
  if (!['install', 'update'].includes(action) || !rawName || !harness) {
    console.error('Usage: bro harness <install|update> <claude|omp|pi|codex|dsh>');
    return 1;
  }
  try {
    const result = action === 'update' ? updateHarnessTool(harness) : ensureHarnessTool(harness);
    if (harness === 'dsh') ensureDshProfilesPlugin();
    console.log(`${HARNESS_INSTALLS[harness].label} is ready: ${result.executable}`);
    return 0;
  } catch (error) {
    console.error(`${HARNESS_INSTALLS[harness]?.label || rawName} ${action} failed: ${error.message}`);
    return 1;
  }
}

function configuredProviderKeys(providers, config) {
  return Object.fromEntries(providers.flatMap((provider) => {
    const key = config.keys?.[provider.id]
      || (provider.keyEnv ? process.env[provider.keyEnv] : '')
      || (provider.mode === 'native' && provider.id === 'anthropic' ? process.env.ANTHROPIC_API_KEY : '')
      // A relay user may only ever have per-tier tokens and no plain key. The
      // harnesses bro syncs (omp, Pi, DSH) take one key per provider, so give
      // them a token that at least reaches part of the catalogue rather than
      // leaving the provider unauthenticated. The run bro launches itself
      // replaces this with the token for the tier actually chosen.
      || Object.entries(config.keys || {}).find(([k, v]) => v && k.startsWith(`${provider.id}@`))?.[1]
      || '';
    return key ? [[provider.id, key]] : [];
  }));
}

export async function main(argv) {
  if (argv[0] === 'harness') return runHarnessCommand(argv.slice(1));
  if (argv[0] === 'update' && argv[1] && normalizeHarness(argv[1])) {
    return runHarnessCommand(['update', argv[1]]);
  }
  if (['tokens', 'token-report'].includes(argv[0])) {
    return runTokenReport();
  }
  if (['profiles', 'profiles-report'].includes(argv[0])) {
    return runProfilesReport();
  }
  if (argv[0] === 'accounts') {
    return runPoolAccounts(argv.slice(1));
  }
  if (argv[0] === 'browser') {
    return runClaudeBrowserCommand(argv.slice(1));
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

  // `bro service …` is shorthand for the JustImagine background service — it
  // is the only thing bro runs as one.
  if (argv[0] === 'service' || argv[0] === 'daemon') {
    ensureDefaultConfig();
    return runServiceCommand(argv.slice(1));
  }

  // `bro imagine <sub-command>`: service control, `open`, and its own help.
  // Anything else falls through to opening the gallery below.
  if (isImagineWord(argv[0]) && argv[1]) {
    ensureDefaultConfig();
    const handled = await runImagineCommand(argv.slice(1), { config: loadConfig() });
    if (handled !== null) return handled;
  }

  // Help dispatch: `bro help [topic]` and `bro imagine help|-h|--help`.
  if (argv[0] === 'help' || (isImagineWord(argv[0]) && isHelpWord(argv[1]))) {
    if (isImagineWord(argv[0]) || isImagineWord(argv[1])) {
      ensureDefaultConfig();
      console.log(imagineHelp(loadConfig()));
    } else {
      console.log(HELP);
    }
    return 0;
  }

  const args = parseArgs(argv);
  if (args.help) { console.log(HELP); return 0; }
  if (args.version) { console.log(pkg.version); return 0; }

  if (args.update) {
    let code = 0;
    try {
      const r = await updateModels();
      console.log(`Updated models from ${r.source}`);
      console.log(`  ${r.providers} providers · ${r.models} models`);
      console.log(`  stored at ${r.cache}`);
    } catch (e) {
      console.error(`Update failed: ${e.message}`);
      console.error('Kept the existing local copy.');
      code = 1;
    }
    // The OpenRouter catalogue (ages, prices, benchmark scores) and the
    // per-model speed measurements have their own caches; refresh both here so
    // the picker never has to wait for them.
    const live = await loadOpenRouterModels();
    if (!live) {
      console.error('OpenRouter catalogue: could not be fetched.');
      return code || 1;
    }
    console.log(`  OpenRouter catalogue: ${live.length} models`);
    const apiKey = openRouterKey(loadConfig());
    if (!apiKey) {
      console.log('  \x1b[2mSpeed ratings need an OpenRouter key (OPENROUTER_API_KEY, or pick OpenRouter once and save one).\x1b[0m');
      return code;
    }
    const ids = live.map((m) => m.id);
    const start = Date.now();
    const progress = (stats) => {
      if (!isInteractive) return;
      const n = ids.filter((id) => (stats[id]?.at || 0) >= start).length;
      process.stderr.write(`\r\x1b[2K  \x1b[2mmeasuring speed: ${n}/${ids.length}\x1b[0m`);
    };
    const stats = await refreshOpenRouterStats({ ids, apiKey, concurrency: 6, maxAge: 0, onUpdate: progress });
    if (isInteractive) process.stderr.write('\r\x1b[2K');
    const measured = ids.filter((id) => stats[id]?.tps != null).length;
    console.log(`  speed measured for ${measured}/${ids.length} models`);
    return code;
  }

  ensureDefaultConfig();
  const config = loadConfig();
  // Flags win, then the harness used last time (the menu toggle is sticky),
  // then the configured default.
  let harness = normalizeHarness(args.harness || lastHarness() || config.defaultHarness || 'claude');
  if (!harness) {
    console.error(`Unknown harness: ${args.harness || config.defaultHarness}  (use: claude, omp, pi, codex or dsh)`);
    return 1;
  }

  // --print is Claude Code's print mode under bro's own name, so it turns into
  // the harness's own flag and everything else the user passed still follows.
  // Checked before the harness is remembered: a combination bro is about to
  // refuse must not become the default for every later run.
  if (args.print !== undefined) {
    if (harness !== 'claude') {
      console.error(`--print runs Claude Code headless; the ${harness} harness has its own way of doing that.`);
      console.error('  Drop --harness, or pass that harness\'s flags after --.');
      return 1;
    }
    // Claude Code takes the prompt as a positional, and its own parser reads a
    // leading dash as a flag however it is quoted. stdin has no such problem.
    if (args.print.startsWith('-')) {
      console.error('A prompt starting with "-" cannot be passed as an argument — Claude Code reads it as a flag.');
      console.error('  Pipe it in instead:');
      console.error(`    echo ${JSON.stringify(args.print)} | bro ${args.provider ? `-p ${args.provider} ` : ''}--print`);
      return 1;
    }
    args._ = ['-p', ...(args.print ? [args.print] : []), ...args._];
  }
  const headless = isHeadlessRun({ print: args.print, harnessArgs: args._ });
  // What the picker reopens on next time is a record of what you chose, so
  // only a choice writes it. A dry run chose nothing, and a headless run is a
  // script doing a job — neither should quietly redecide your default.
  const persistChoice = !args.dryRun && !headless;

  // A command-line harness is already a completed selection. Persist it now,
  // even when setup or provider authentication later fails.
  if (persistChoice && !args.imagine && args.harness) rememberHarness(harness);

  // `bro imagine` goes straight to the JustImagine gallery (no harness involved).
  if (args.imagine) {
    const flags = parseImagineArgs(argv.slice(1));
    return runJustImagine({
      config,
      apiId: flags.api || args.provider,
      dryRun: args.dryRun,
      root: flags.root,
      port: flags.port,
      open: flags.open !== false,
      authRequired: false,
      loadConfigFn: loadConfig,
      setKeyFn: setKey,
      configPathValue: CONFIG_PATH
    });
  }

  const data = await loadModels();
  // The account pool and JustImagine are always pinned on top — no models.json entry needed.
  const providers = [IMAGINE_PROVIDER, POOL_PROVIDER, ACCOUNT_PROVIDER, CODEX_PROVIDER, ...mergeProviders(data, config.providers)];

  if (!providers.length) {
    console.error('No providers available. Check your network or ~/.bro/config.json.');
    return 1;
  }

  if (args.list) {
    for (const p of providers) {
      console.log(`\n${p.name || p.id}  \x1b[2m(${p.id} · ${tagOf(p)})\x1b[0m`);
      // A relay's real list is its live catalogue, not the handful of names
      // models.json carries as an offline fallback — say so rather than let
      // six rows look like the whole shop.
      if (isNewApi(p)) {
        const cached = readNewApiCache(p.id);
        const rows = cached?.models || p.models || [];
        console.log(`  \x1b[2m${rows.length} models${cached ? '' : ' (offline fallback — run bro -p ' + p.id + ' to fetch the catalogue)'} · each with its own tiers\x1b[0m`);
        for (const m of rows) {
          const tiers = (m.tiers || []).map((t) => t.id);
          console.log(`  - ${m.id}${tiers.length ? `  \x1b[2m${tiers.join(', ')}\x1b[0m` : ''}`);
        }
        continue;
      }
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
    const apiKey = args.dryRun ? '' : openRouterKey(config);
    // Rows for a model list, and — with an OpenRouter key — a background pass
    // that measures the models' speed and repaints the rows as numbers arrive.
    // Newest models are queued first because that is what the column shows
    // first; the pass stops when the picker closes.
    const liveRows = (models, p, { update, signal }, { measure = true } = {}) => {
      // Only ids OpenRouter knows can be measured: its own catalogue's, or the
      // catalogue id a match from another provider carries.
      const ids = models.map((m) => (p.id === 'openrouter' ? m.id : m.catalogueId)).filter(Boolean);
      if (measure && apiKey && ids.length) {
        refreshOpenRouterStats({
          ids,
          apiKey,
          signal,
          limit: 160,
          onUpdate: (stats) => update(modelRows(attachStats(models, stats), p))
        }).catch(() => {});
      }
      return modelRows(models, p);
    };
    const childrenFor = (p) => {
      if (p.mode === 'imagine') {
        return mergeImageApis(config.imageApis).map((a) => ({
          label: `${a.name || a.id}  \x1b[2m${a.models?.[0]?.id || ''}\x1b[0m`,
          value: a.id
        }));
      }
      // OpenRouter's live catalogue: a copy fetched in the last few hours is
      // shown as-is, otherwise it is fetched now (falling back to the last
      // copy, then to the static list).
      if (p.id === 'openrouter') {
        return async (ctx) => liveRows(attachStats((await loadOpenRouterModels({ maxAge: CATALOGUE_FRESH_MS })) || p.models || []), p, ctx);
      }
      // Account profiles with live usage stats (5h/week/Fable) in the right
      // pane, followed by the sessions those profiles can resume.
      if (p.mode === 'account') return accountProfileChoices;
      // Codex logins with the sessions they can resume — the same shape as the
      // account pane. (Its models come from the subscription and are chosen
      // after the login, not here.)
      if (p.mode === 'codex') return codexProfileChoices;
      // A new-api relay publishes its entire catalogue unauthenticated, so the
      // column can browse all of it. Each row is priced at that model's
      // cheapest tier — the tier menu after the pick shows what the other
      // upstream routes charge for the same thing.
      if (isNewApi(p)) {
        return async (ctx) => {
          const cached = readNewApiCache(p.id);
          if (!cached || cached.age > CATALOGUE_FRESH_MS) {
            loadNewApiCatalogue({ id: p.id, baseUrl: p.baseUrl, signal: ctx.signal })
              .then((live) => {
                if (live && !ctx.signal.aborted) ctx.update(liveRows(enrichModels(live, readOpenRouterCache()?.models), p, ctx));
              })
              .catch(() => {});
          }
          return liveRows(enrichModels(cached?.models || p.models || [], readOpenRouterCache()?.models), p, ctx, { measure: false });
        };
      }
      if (!(p.models || []).length) return null;
      // Every other provider's models are annotated from the OpenRouter
      // catalogue. The rows appear at once from whatever copy is on disk; a
      // stale or missing copy is refreshed behind them.
      return async (ctx) => {
        const cached = readOpenRouterCache();
        if (cached && cached.age <= CATALOGUE_FRESH_MS) return liveRows(enrichModels(p.models, cached.models), p, ctx);
        loadOpenRouterModels()
          .then((live) => {
            if (live && !ctx.signal.aborted) ctx.update(liveRows(enrichModels(p.models, live), p, ctx));
          })
          .catch(() => {});
        return liveRows(enrichModels(p.models, cached?.models), p, ctx, { measure: false });
      };
    };
    // Providers that are ready to launch (key saved / env var / no key needed)
    // go on top in green, the rest below a divider.
    // A relay's token is created for one tier, so a user who only ever uses
    // Codex-Gpt-1 has `openlux@Codex-Gpt-1` and no plain `openlux` key. That
    // still counts as configured.
    const hasKey = (id, keyEnv) =>
      Boolean(
        config.keys?.[id]
        || (keyEnv && process.env[keyEnv])
        || Object.entries(config.keys || {}).some(([k, v]) => v && k.startsWith(`${id}@`))
      );
    const isConfigured = (p) => {
      if (p.mode === 'imagine') return mergeImageApis(config.imageApis).some((a) => hasKey(a.id, a.keyEnv));
      if (p.mode === 'native' || p.noKey || ['pool', 'account', 'codex'].includes(p.mode)) return true;
      return hasKey(p.id, p.keyEnv);
    };
    const toChoice = (p, configured) => ({
      label: p.name || p.id,
      value: p,
      color: configured ? '\x1b[32m' : '',
      detail: tagOf(p),
      children: childrenFor(p),
      filterableChildren: p.id === 'openrouter' || isNewApi(p) || p.mode === 'account' || p.mode === 'codex',
      // Codex's pane lists logins rather than models, so it reopens on the
      // login used last.
      childValue: p.mode === 'codex' ? lastProfileFor(p.id) : lastModelFor(p.id)
    });
    // Aggregators and relays go in their own labelled group at the bottom.
    // They resell the same model names as the first-party providers above, so
    // mixing them into one list makes "claude-opus-5" ambiguous.
    const firstParty = providers.filter((p) => p.section !== 'other');
    const other = providers.filter((p) => p.section === 'other');
    const ready = firstParty.filter((p) => isConfigured(p));
    const rest = firstParty.filter((p) => !isConfigured(p));
    const choices = [
      ...ready.map((p) => toChoice(p, true)),
      ...(ready.length && rest.length ? [{ divider: true }] : []),
      ...rest.map((p) => toChoice(p, false)),
      ...(other.length ? [{ divider: true, label: 'Other Providers' }] : []),
      ...other.map((p) => toChoice(p, isConfigured(p)))
    ];

    const lastP = lastProvider();
    const browser = browserToggle();
    const choice = await selectColumns({
      message: 'Choose a provider and model:',
      startIndex: Math.max(0, choices.findIndex((c) => c.value?.id === lastP)),
      choices,
      clearScreen: true,
      banner: BANNER,
      toggle: { label: 'Skip permissions', value: skip },
      toggles: [HARNESS_TOGGLE(harness), ...(browser ? [browser] : [])]
    }).catch(() => null);
    if (!choice) {
      if (headless) { explainNoMenu('a provider and model'); return 1; }
      console.log('Cancelled.');
      return 0;
    }
    provider = choice.value;
    picked = choice;
    if (choice.toggleOn !== undefined) skip = choice.toggleOn;
    if (choice.toggles?.harness) harness = choice.toggles.harness;
    if (!args.dryRun) {
      rememberHarness(harness);
      applyBrowserToggle(choice.toggles?.browser);
    }
  }

  // JustImagine: the picker's right column already chose the image API (falls
  // back to its own menu when it didn't), then serve the local gallery.
  // Deliberately not remembered as the default provider — it's the exception.
  if (provider.mode === 'imagine') {
    return runJustImagine({ config, apiId: picked?.child?.value, dryRun: args.dryRun, authRequired: false, loadConfigFn: loadConfig, setKeyFn: setKey, configPathValue: CONFIG_PATH });
  }

  // DSH has its own provider/model switcher, so give it the complete live
  // OpenRouter catalog even when OpenRouter was not the row browsed in bro.
  // Every other provider already carries its complete merged model list.
  if (harness === 'dsh' && !args.dryRun) {
    const openrouter = providers.find((entry) => entry.id === 'openrouter');
    if (openrouter) {
      const live = await loadOpenRouterModels();
      if (live?.length) openrouter.models = live;
    }
  }
  const providerKeys = configuredProviderKeys(providers, config);

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
      headless,
      dryRun: args.dryRun,
      providers,
      providerKeys
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
  if (provider.id === 'openrouter' && !args.dryRun && (model == null || harness === 'omp' || harness === 'dsh')) {
    if (isInteractive) process.stderr.write('\x1b[2mFetching OpenRouter models…\x1b[0m\r');
    const live = await loadOpenRouterModels();
    if (isInteractive) process.stderr.write('\x1b[2K');
    if (live) provider.models = live;
  }

  // A relay's rows carry the two things launching needs and models.json cannot
  // hold: every tier that serves the model, and which protocol that model
  // speaks. The picker column already fetched them; a -p/-m run has not, so do
  // it here. Only a fetch that fails outright leaves the bundled fallback list.
  if (isNewApi(provider)) {
    const stale = !readNewApiCache(provider.id);
    if (isInteractive && stale) process.stderr.write(`\x1b[2mFetching the ${provider.name || provider.id} catalogue…\x1b[0m\r`);
    const live = await loadNewApiCatalogue({
      id: provider.id,
      baseUrl: provider.baseUrl,
      maxAge: CATALOGUE_FRESH_MS,
      fallback: provider.models
    });
    if (isInteractive && stale) process.stderr.write('\x1b[2K');
    if (live?.length) provider.models = live;
  }

  const models = provider.models || [];
  if (model == null) {
    if (harness === 'omp' || !models.length) {
      model = '';
    } else if (headless) {
      // Nothing to pick with, so take what the menu offers first — the same
      // row pressing Enter would have taken. For Claude's own provider that
      // row is "your login's default", which carries no model id at all.
      model = models[0].id ?? '';
      if (models.length > 1) note(`\x1b[2mNo -m given; using ${model || 'the default model'}.\x1b[0m`);
    } else {
      const lastM = lastModelFor(provider.id);
      // OpenRouter's rows already carry their facts; other providers' rows are
      // annotated from the catalogue copy on disk.
      const annotated = provider.id === 'openrouter' ? attachStats(models) : enrichModels(models, readOpenRouterCache()?.models);
      const choice = await select({
        message: `Choose a model for ${provider.name || provider.id}:`,
        header: modelHeaderFor(annotated, provider),
        startIndex: lastM != null ? Math.max(0, models.findIndex((m) => (m.id ?? '') === lastM)) : 0,
        // Drop the heading divider: the single-column list has its own header slot.
        choices: modelRows(annotated, provider).slice(1),
        filterable: provider.id === 'openrouter' || isNewApi(provider),
        toggle: { label: 'Skip permissions', value: skip },
        toggles: [HARNESS_TOGGLE(harness)]
      }).catch(() => null);
      if (choice == null) {
        if (headless) { explainNoMenu(`a model for ${provider.name || provider.id}`); return 1; }
        console.log('Cancelled.');
        return 0;
      }
      model = choice.value;
      if (choice.toggleOn !== undefined) skip = choice.toggleOn;
      if (choice.toggles?.harness) harness = choice.toggles.harness;
      if (!args.dryRun) rememberHarness(harness);
    }
  }

  // 2b) tier — which upstream route at a new-api relay serves this model. The
  // relay resells one model through several: an official API key, an Azure
  // deployment, a subscription client. They answer to the same model name and
  // differ by an order of magnitude in price, so this is a real choice rather
  // than a detail, and bro makes it per model because the routes on offer
  // differ per model.
  //
  // The tier is fixed when the token is created in the relay's console — it
  // cannot be set per request — so the chosen tier also chooses which saved
  // token to launch with (see keySlot below).
  let tier = '';
  let relayModel = null;
  if (isNewApi(provider) && model) {
    relayModel = modelById(provider.models, model);
    const tiers = relayModel?.tiers || [];
    if (args.tier) {
      const match = tiers.find((t) => t.id.toLowerCase() === args.tier.toLowerCase());
      if (!match && tiers.length) {
        console.error(`${provider.name || provider.id} does not serve ${model} through "${args.tier}".`);
        console.error(`  Tiers for ${model}: ${tiers.map((t) => t.id).join(', ')}`);
        return 1;
      }
      tier = match?.id || args.tier;
    } else if (tiers.length === 1) {
      tier = tiers[0].id;
    } else if (tiers.length) {
      const remembered = lastTierFor(provider.id, model);
      const start = Math.max(0, tiers.findIndex((t) => t.id === remembered));
      if (headless || args.dryRun) {
        // Nothing to pick with: take what the menu would have offered first —
        // the tier used last for this model, else the cheapest one.
        tier = tiers[start].id;
        if (!args.dryRun) note(`\x1b[2mNo --tier given; using ${tier} (${priceLabel(tiers[start].pricing)}).\x1b[0m`);
      } else {
        // Which tiers already have a token of their own — an empty entry in
        // config.json is a placeholder, not a key.
        const keyed = new Set(Object.entries(config.keys || {}).filter(([, v]) => v).map(([k]) => k));
        const cheapest = tiers[0].ratio;
        const choice = await select({
          message: `Choose a tier for ${model} at ${provider.name || provider.id}:`,
          header: (width) => tierHeader({ width }),
          startIndex: start,
          choices: tiers.map((t) => ({
            label: (width) => tierRow(t, { width, keyed: keyed.has(tierKeyId(provider.id, t.id)), best: cheapest }),
            value: t.id,
            filterText: `${t.id} ${t.label || ''}`
          })),
          filterable: true,
          toggle: { label: 'Skip permissions', value: skip },
          toggles: [HARNESS_TOGGLE(harness)]
        }).catch(() => null);
        if (choice == null) {
          console.log('Cancelled.');
          return 0;
        }
        tier = choice.value;
        if (choice.toggleOn !== undefined) skip = choice.toggleOn;
        if (choice.toggles?.harness) harness = choice.toggles.harness;
        if (!args.dryRun) rememberHarness(harness);
      }
    }
    if (persistChoice && tier) rememberTier(provider.id, model, tier);
    // Price the row at the tier actually chosen, then point the launcher at the
    // protocol this model speaks: Claude models take Claude Code straight to
    // /v1/messages, everything else goes through the OpenAI proxy.
    if (relayModel) {
      relayModel = withTier(relayModel, tier);
      provider = providerForModel(provider, relayModel);
    }
  }

  // Account pool: its own setup → start proxy → launch the selected harness
  // against the local Anthropic-compatible pool endpoint.
  if (provider.mode === 'pool') {
    if (persistChoice) rememberSelection(provider.id, model, harness);
    const result = await runPool({
      model,
      extraArgs: args._,
      skipPermissions: skip,
      harness,
      providers,
      providerKeys,
      headless,
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
    if (harness === 'dsh') {
      if (session) {
        console.error('A Claude Code transcript cannot be resumed inside DeepSeek Harness.');
        console.error('  Choose a Claude profile instead; DSH keeps its own resumable sessions.');
        return 1;
      }
      const fallback = providers.find((entry) => entry.id === 'anthropic' && entry.mode === 'native')
        || providers.find((entry) => ['native', 'anthropic', 'openai'].includes(entry.mode));
      if (!fallback) {
        console.error('No DSH-compatible provider is configured.');
        return 1;
      }
      if (persistChoice) rememberSelection(provider.id, accountName, harness);
      const result = await launch({
        provider: fallback,
        model: '',
        apiKey: providerKeys[fallback.id] || '',
        providers,
        providerKeys,
        extraArgs: args._,
        skipPermissions: skip,
        harness,
        dryRun: args.dryRun,
        preferredProfile: { kind: 'claude', name: accountName || 'local' }
      });
      if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
      return typeof result === 'number' ? result : 0;
    }
    // Remember the account (not a model) so the picker preselects it next time.
    if (persistChoice && !session) rememberSelection(provider.id, accountName);
    const result = await runAccountProfile({
      accountName,
      model,
      session,
      extraArgs: args._,
      skipPermissions: skip,
      headless,
      dryRun: args.dryRun
    });
    if (args.dryRun) { console.log(JSON.stringify(result, null, 2)); return 0; }
    return typeof result === 'number' ? result : 0;
  }

  // 3) key (skipped for native Claude and noKey/local providers)
  //
  // A relay's token carries its tier, so each tier gets its own saved slot
  // ("openlux@Codex-Gpt-1"). The provider's plain key stays the fallback: one
  // token still launches every tier it happens to be allowed to reach.
  const keySlot = tier ? tierKeyId(provider.id, tier) : provider.id;
  const keyFor = tier ? `${provider.name || provider.id} · ${tier}` : provider.name || provider.id;
  let apiKey = '';
  if (provider.mode !== 'native' && !provider.noKey) {
    apiKey = tier
      ? newApiKey({ providerId: provider.id, tier, config, keyEnv: provider.keyEnv })
      : (config.keys && config.keys[provider.id]) ||
        (provider.keyEnv && process.env[provider.keyEnv]) ||
        '';
    if (!apiKey && !args.dryRun) {
      // A headless run has nowhere to type a key, and the two places it could
      // have come from are worth naming rather than reporting an empty answer.
      if (headless) {
        console.error(`No API key for ${keyFor}, and this run cannot prompt for one.`);
        if (provider.keyEnv) console.error(`  Set ${provider.keyEnv}, or save it once with an interactive "bro -p ${provider.id}".`);
        else console.error(`  Add it under "keys" in ${CONFIG_PATH}.`);
        if (tier) console.error(`  A relay token is created for one tier — this one needs a token made for "${tier}".`);
        if (provider.keyUrl) console.error(`  Get one: ${provider.keyUrl}`);
        return 1;
      }
      const hint = provider.keyUrl ? `  \x1b[2m(get one: ${provider.keyUrl})\x1b[0m` : '';
      if (tier) note(`\x1b[2mThe token must be one you created for the "${tier}" group — the tier cannot be set per request.\x1b[0m`);
      apiKey = await promptHidden(`Enter API key for ${keyFor}${hint}\n> `).catch(() => '');
      if (!apiKey) { console.error('No key entered.'); return 1; }
      setKey(keySlot, apiKey);
      note(`Saved to ${CONFIG_PATH}`);
    }
  }

  if (persistChoice) rememberSelection(provider.id, model, harness);

  if (apiKey) providerKeys[provider.id] = apiKey;

  // Say which of the relay's routes this run is about to buy from, and at what
  // price — the same model through a different tier is a different bill.
  if (tier && !args.dryRun) {
    const price = priceLabel(relayModel?.pricing || (relayModel?.perCall != null ? { perCall: relayModel.perCall } : null));
    note(`\x1b[2mTier ${tier}${price ? ` · ${price}` : ''} · ${provider.mode === 'anthropic' ? 'anthropic-compatible' : 'via proxy'}\x1b[0m`);
  }

  const result = await launch({
    provider,
    model,
    apiKey,
    providers,
    providerKeys,
    extraArgs: args._,
    skipPermissions: skip,
    harness,
    headless,
    dryRun: args.dryRun
  });

  if (args.dryRun) {
    console.log(JSON.stringify(tier ? { ...result, tier, price: priceLabel(relayModel?.pricing) || undefined } : result, null, 2));
    return 0;
  }
  return typeof result === 'number' ? result : 0;
}
