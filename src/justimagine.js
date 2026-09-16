import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { loadConfig, setKey, CONFIG_PATH } from './config.js';
import { loadDesignArena, loadOpenRouterImageModels, loadOpenRouterVideoModels } from './models.js';
import { select, promptHidden, isInteractive } from './ui.js';
import { rememberModelFor, lastModelFor } from './state.js';
import { mergeImageApis, IMAGE_APIS, VIDEO_KEY_API } from './justimagine-gen.js';
import { createServer, listenOnFreePort } from './justimagine-server.js';
import { migrateLegacy } from './justimagine-store.js';
import { CHARACTERS_DIR } from './justimagine-characters.js';
import {
  DEFAULT_PORT,
  SERVICE_ERR,
  SERVICE_LOG,
  backendFor,
  defaultServiceRoot,
  elevationHint,
  isElevated,
  probe,
  readServiceConfig,
  serviceInstall,
  serviceStart,
  serviceStatus,
  serviceStop,
  serviceUninstall,
  writeServiceConfig
} from './justimagine-service.js';

export { mergeImageApis, IMAGE_APIS };

// The provider-menu entry that routes into this flow (mode 'imagine' is handled
// in cli.js before any harness launching logic).
export const IMAGINE_PROVIDER = { id: 'justimagine', name: '✦ JustImagine — images & video', mode: 'imagine', models: [] };

// ---------- logo ----------

// Drenched in the brand gold rather than ramped across it: the house style
// commits to one yellow on a brand surface, it doesn't gradient.
const BRAND = '\x1b[38;5;220m';
const RESET = '\x1b[0m';

const WORDMARK = [
  ' ╦ ╦ ╦ ╔═╗ ╔╦╗   ╦ ╔╦╗ ╔═╗ ╔═╗ ╦ ╔╗╔ ╔═╗',
  ' ║ ║ ║ ╚═╗  ║    ║ ║║║ ╠═╣ ║ ╦ ║ ║║║ ║╣ ',
  '╚╝ ╚═╝ ╚═╝  ╩    ╩ ╩ ╩ ╩ ╩ ╚═╝ ╩ ╝╚╝ ╚═╝'
];

export function logo({ color = true, tagline = 'images · video · one gallery' } = {}) {
  const lines = WORDMARK.map((l) => (color ? BRAND + l + RESET : l));
  const sub = color ? `\x1b[2m         ✦  ${tagline}${RESET}` : `         ✦  ${tagline}`;
  return ['', ...lines, sub, ''].join('\n');
}

// ---------- roots ----------

export const LOCAL_ROOT = ['.bro', 'justimagine'];
const legacyRoots = (cwd) => [path.join(cwd, '.bro', 'image-gen'), path.join(cwd, '.bro', 'context')];

// Where a plain `bro imagine` keeps its gallery: beside the project, the way
// the old image-gen did. The service defaults somewhere durable instead.
export function localRoot(cwd = process.cwd()) {
  return path.join(cwd, ...LOCAL_ROOT);
}

// ---------- help ----------

export function imagineHelp(config = {}) {
  const apis = mergeImageApis(config.imageApis);
  const width = Math.max(...apis.map((a) => a.id.length), 8);
  const apiLines = apis
    .map((a) => {
      const models = (a.models || []).map((m) => m.id).join(', ') || '(no models listed)';
      const tag = a.video ? '  \x1b[38;5;214m+ video\x1b[0m' : '';
      return `  ${a.id.padEnd(width)}  ${a.name || a.id}${tag}\n  ${' '.repeat(width)}  \x1b[2m${models}\x1b[0m`;
    })
    .join('\n');

  return `${logo()}
bro imagine — generate images and video from a local web UI.

Usage:
  bro imagine              Pick an API, then open the gallery
  bro imagine -p <api>     Skip the API menu (e.g. bro imagine -p openrouter)
  bro imagine --root <dir> Use a different gallery folder
  bro imagine --port <n>   Serve on a fixed port
  bro imagine open         Open the running service in your browser
  bro imagine service …    Run it in the background, always (see below)
  bro imagine help         Show this help

Images:
  Any OpenAI-shaped /images/generations API, plus the chat-routed image
  models (Gemini / Nano Banana, GPT-5 Image) that aggregators serve
  through /chat/completions.

Video:
  OpenRouter's video API — Veo 3.1, Sora 2 Pro, Seedance 2.x, Wan 3.0,
  Kling v3, Hailuo, Runway Gen-4.5, Grok Imagine and the rest of the
  catalogue, refreshed live. Each model's real duration / resolution /
  aspect-ratio / audio options drive the controls, so you can only ask
  for combinations that model actually accepts. Attach a reference image
  and a model that supports first-frame conditioning animates it.

Model picker:
  Every model row rates the model on age, cost, speed and quality. Age and
  cost come from OpenRouter (an estimated price per image, or the list
  price per second of video); speed is how long that model has taken in
  this gallery; quality is the model's Design Arena rank (image models).
  A blank cell means nothing is known yet.

Folders:
  The sidebar is the folder tree of your gallery root, and everything you
  generate lands in the folder you have selected. Make folders, rename
  them, drag generations between them. Deleting a folder deletes every
  generation inside it.

Better prompts:
  The ✨ beside either text box rewrites what you wrote into something the
  picked model can work with — composition and light for an image, a named
  camera move for a clip, the permanent look for a character. It runs on
  google/gemini-3.7-flash through your OpenRouter key. ↺ puts back exactly
  what you typed.

Characters:
  A repeatable cast, shared by every gallery. Give a character a name, a
  description and a few reference images, then pick it when you generate:
  its pictures ride along with the prompt and it is named in the text, so
  the same face comes back shot after shot. A generation you liked can be
  promoted into one of its references, which is how a character sharpens
  over time. Stored in ~/.bro/justimagine/characters.

  No photos to start from? "Draw 5 reference shots" builds a sheet with
  Nano Banana 2: one portrait from the description, then four more angles
  drawn *from that portrait* — which is what makes them one character
  rather than five people who match the same sentence. Pick the ones you
  want; the rest are thrown away.

APIs:
${apiLines}

  Keys are shared with the chat provider of the same id, so a saved
  yunwu key just works. Video always uses the ${VIDEO_KEY_API} key.
  Add more APIs via the "imageApis" array in your config file.

Background service:
  bro imagine service install [--root <dir>] [--port <n>]
                           Serve this directory's gallery, from now on
  bro imagine service status | start | stop | uninstall | logs
  Unelevated it runs while you are logged on; elevated (sudo / an
  Administrator terminal) it starts with the machine and replaces an
  unelevated install automatically.
  ${serviceMechanismLine()}

For agents:
  bro imagine skill        Install the generate-images-videos skill into
                           ./.claude/skills (--global for ~/.claude/skills)
  The gallery's own JSON API drives everything the page does — POST
  /api/batch queues many images and clips at once, GET /api/jobs?wait=
  blocks until they land. See docs/justimagine-api.md.

Files:
  Gallery:  ./${LOCAL_ROOT.join('/')}   (per-folder history.jsonl beside the media)
  Refs:     <gallery>/.context
  Thumbs:   <gallery>/.thumbs           (derived; safe to delete)
  Cast:     ~/.bro/justimagine/characters  (global, shared by every gallery)

Ctrl-C stops a foreground server.

Config:  ${CONFIG_PATH}`;
}

function serviceMechanismLine() {
  const backend = backendFor();
  if (!backend) return `\x1b[2mNo service integration for ${process.platform}.\x1b[0m`;
  return `\x1b[2mUses ${backend.name} on this machine — no admin needed for a per-user install.\x1b[0m`;
}

// ---------- shared startup ----------

function openBrowser(url) {
  try {
    if (process.platform === 'win32') {
      spawn(process.env.ComSpec || 'cmd.exe', ['/d', '/s', '/c', `start "" "${url}"`], {
        windowsVerbatimArguments: true,
        detached: true,
        stdio: 'ignore'
      }).unref();
    } else if (process.platform === 'darwin') {
      spawn('open', [url], { detached: true, stdio: 'ignore' }).unref();
    } else {
      spawn('xdg-open', [url], { detached: true, stdio: 'ignore' }).unref();
    }
  } catch {
    /* the printed URL is enough */
  }
}

// Keys are read per call rather than captured at boot, so a key saved into the
// config while the service is running is picked up without a restart.
function keyResolver(config) {
  let cached = config;
  let readAt = Date.now();
  return (apiId) => {
    if (Date.now() - readAt > 5000) {
      cached = loadConfig();
      readAt = Date.now();
    }
    const api = mergeImageApis(cached.imageApis).find((a) => a.id === apiId);
    return (cached.keys && cached.keys[apiId]) || (api?.keyEnv && process.env[api.keyEnv]) || '';
  };
}

// Live catalogues, in parallel, with the cached/bundled copies as fallback so a
// slow or absent network never blocks the gallery from opening.
async function loadCatalogues(apis, { quiet = false } = {}) {
  const openrouter = apis.find((a) => a.id === 'openrouter');
  if (!quiet && isInteractive) process.stdout.write('\x1b[2mFetching model catalogues…\x1b[0m\r');
  const [images, videos, arena] = await Promise.all([
    openrouter ? loadOpenRouterImageModels() : Promise.resolve(null),
    loadOpenRouterVideoModels(),
    // The quality ranking for both media, taken from the board itself rather
    // than OpenRouter's partial snapshot of it. Never blocks the gallery: it
    // falls back to its cache, and then to no ranking at all.
    loadDesignArena()
  ]);
  if (!quiet && isInteractive) process.stdout.write('\x1b[2K');
  if (openrouter && images) openrouter.models = images;
  return { videoModels: videos || [], designArena: arena || null };
}

async function startServer({ root, apis, videoModels, designArena, resolveKey, defaultApi, port, fixedPort, auth = false, charactersRoot = CHARACTERS_DIR }) {
  // The gallery's settings panel writes keys straight into ~/.bro/config.json,
  // the same file and the same helper the CLI's key prompt uses, so a key added
  // in either place shows up in both. keyResolver re-reads every few seconds,
  // so a saved key takes effect without a restart.
  const server = createServer({
    root,
    apis,
    videoModels,
    designArena,
    resolveKey,
    defaultApi,
    charactersRoot,
    saveKey: setKey,
    configPath: CONFIG_PATH,
    statsRefresh: true,
    auth
  });
  const listenPort = fixedPort ? await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => resolve(port));
  }) : await listenOnFreePort(server, port);
  return { server, port: listenPort, url: `http://127.0.0.1:${listenPort}` };
}

function waitForSignals(server, onStop) {
  return new Promise((resolve) => {
    let stopping = false;
    const stop = () => {
      if (stopping) return;
      stopping = true;
      onStop?.();
      server.jobs.closeAll();
      server.close(() => resolve(0));
      server.closeAllConnections?.();
      // Don't let a lingering keep-alive socket hold the process open.
      setTimeout(() => resolve(0), 1500).unref();
    };
    process.on('SIGINT', stop);
    process.on('SIGTERM', stop);
  });
}

// ---------- `bro imagine` ----------

export async function runJustImagine({ config, apiId, dryRun = false, root: rootArg, port: portArg, open = true } = {}) {
  const apis = mergeImageApis(config.imageApis);

  let api;
  if (apiId) {
    api = apis.find((a) => a.id === apiId || (a.name || '').toLowerCase() === String(apiId).toLowerCase());
    if (!api) {
      console.error(`Unknown API: ${apiId}  (available: ${apis.map((a) => a.id).join(', ')})`);
      return 1;
    }
  } else if (apis.length === 1 || !isInteractive) {
    api = apis[0];
  } else {
    const last = lastModelFor(IMAGINE_PROVIDER.id);
    const width = Math.max(...apis.map((a) => (a.name || a.id).length));
    const choice = await select({
      message: 'Choose an image API (video always runs on OpenRouter):',
      startIndex: Math.max(0, apis.findIndex((a) => a.id === last)),
      choices: apis.map((a) => ({
        label: `${(a.name || a.id).padEnd(width)}  \x1b[2m${a.video ? 'images + video' : a.models[0]?.id || ''}\x1b[0m`,
        value: a
      }))
    }).catch(() => null);
    if (!choice) {
      console.log('Cancelled.');
      return 0;
    }
    api = choice.value;
  }

  const root = path.resolve(rootArg || localRoot());

  if (dryRun) {
    console.log(JSON.stringify({ via: 'justimagine', api: api.id, root, port: portArg || 8790 }, null, 2));
    return 0;
  }

  const [legacyOut, legacyCtx] = legacyRoots(process.cwd());
  const migrated = rootArg ? { images: 0, context: 0 } : migrateLegacy(root, legacyOut, legacyCtx);

  const { videoModels, designArena } = await loadCatalogues(apis);

  // Only the chosen API is worth interrupting for. Video prompts separately,
  // in the UI, if OpenRouter has no key yet.
  let apiKey = (config.keys && config.keys[api.id]) || (api.keyEnv && process.env[api.keyEnv]) || '';
  if (!apiKey && isInteractive) {
    const hint = api.keyUrl ? `  \x1b[2m(get one: ${api.keyUrl})\x1b[0m` : '';
    apiKey = await promptHidden(`Enter API key for ${api.name || api.id}${hint}\n> `).catch(() => '');
    if (!apiKey) {
      console.error('No key entered.');
      return 1;
    }
    setKey(api.id, apiKey);
    config = loadConfig();
    console.log(`Saved to ${CONFIG_PATH}`);
  }

  rememberModelFor(IMAGINE_PROVIDER.id, api.id);

  const resolveKey = keyResolver(config);
  const { server, url } = await startServer({
    root,
    apis,
    videoModels,
    designArena,
    resolveKey,
    defaultApi: api.id,
    port: Number(portArg) || 8790,
    fixedPort: !!portArg,
    auth: false
  });

  console.log(logo());
  console.log(`   Gallery:  \x1b[36m${url}\x1b[0m`);
  console.log(`   Folder:   ${root}`);
  console.log(`   Images:   ${api.name || api.id}`);
  console.log(`   Video:    ${resolveKey(VIDEO_KEY_API) ? `OpenRouter · ${videoModels.length} models` : 'OpenRouter key not set — add it to use video'}`);
  if (migrated.images || migrated.context) {
    console.log(`   \x1b[2mMoved ${migrated.images} image${migrated.images === 1 ? '' : 's'} and ${migrated.context} reference${migrated.context === 1 ? '' : 's'} over from .bro/image-gen\x1b[0m`);
  }
  console.log(`   \x1b[2mRun 'bro imagine service install' to keep this running in the background.\x1b[0m`);
  console.log(`   \x1b[2mCtrl-C to stop\x1b[0m\n`);
  if (open) openBrowser(url);

  return waitForSignals(server, () => console.log('\nStopping JustImagine…'));
}

// ---------- service ----------

const SERVICE_HELP = `bro imagine service — keep JustImagine running in the background.

  install [--root <dir>] [--port <n>]   Start now and at every login
  status                                Is it installed / running / reachable
  start | stop                          Control it now
  restart                               Stop then start
  uninstall                             Remove it (your gallery is untouched)
  logs [-n <lines>]                     Tail the service log
  run                                   Run the server in the foreground
                                        (this is what the OS starts)

Where it installs:
  The gallery of the directory you run it in — ./${LOCAL_ROOT.join('/')} — the same
  one 'bro imagine' serves here, unless you pass --root. Install again
  somewhere else and it moves; the old folder is left alone.

Elevated or not:
  Unelevated it starts when you log on and stops when you log out.
  Elevated (sudo / an Administrator terminal) it starts with the machine,
  before anyone logs on, still running as you. An elevated install takes
  priority and replaces an unelevated one automatically; an unelevated
  install leaves a system-wide one alone and says so.

${serviceMechanismLine()}`;

function parseServiceArgs(args) {
  const out = { _: [] };
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (a === '--root') out.root = args[++i];
    else if (a === '--port') out.port = Number(args[++i]);
    else if (a === '-n' || a === '--lines') out.lines = Number(args[++i]);
    else if (a === '--no-open') out.open = false;
    else if (a === '-g' || a === '--global') out.global = true;
    else if (a === '--dir') out.dir = args[++i];
    else out._.push(a);
  }
  return out;
}

// `fresh` is an install: it takes the gallery of the directory you are standing
// in, the same one `bro imagine` serves here. Every other command follows what
// the installed service recorded, so `status` and `logs` work from anywhere.
function serviceSettings(flags = {}, { fresh = false } = {}) {
  const saved = readServiceConfig() || {};
  const root = flags.root || process.env.JUSTIMAGINE_ROOT || (fresh ? '' : saved.root) || defaultServiceRoot();
  return {
    root: path.resolve(root),
    port: Number(flags.port || process.env.JUSTIMAGINE_PORT || saved.port || DEFAULT_PORT)
  };
}

// The long-running process itself. Started by Task Scheduler / launchd /
// systemd, and by `bro imagine service run` when you want to watch it.
async function runService(flags) {
  const cfg = serviceSettings(flags);

  // The scheduled task retries every few minutes, which is the keepalive for a
  // crash. An attempt that finds the gallery already answering is a no-op, not
  // an error worth writing to the log.
  const already = await probe(cfg.port);
  if (already) {
    console.log(`JustImagine is already running at http://127.0.0.1:${cfg.port}  (${already.root})`);
    return 0;
  }

  const config = loadConfig();
  const apis = mergeImageApis(config.imageApis);
  const { videoModels, designArena } = await loadCatalogues(apis, { quiet: true });
  const resolveKey = keyResolver(config);

  // Windows launches this through wscript, which discards stdout — so the
  // service keeps its own log rather than relying on the supervisor's capture.
  const log = (msg) => {
    const line = `${new Date().toISOString()}  ${msg}\n`;
    process.stdout.write(line);
    try {
      fs.mkdirSync(path.dirname(SERVICE_LOG), { recursive: true });
      fs.appendFileSync(SERVICE_LOG, line);
    } catch {
      /* logging must never take the service down */
    }
  };

  const { server, url } = await startServer({
    root: cfg.root,
    apis,
    videoModels,
    designArena,
    resolveKey,
    defaultApi: apis.find((a) => resolveKey(a.id))?.id || apis[0]?.id,
    port: cfg.port,
    // A service must be findable at the port it advertises; if something else
    // has it, say so rather than drifting to a port nobody knows about.
    fixedPort: true,
    auth: false
  }).catch((e) => {
    log(`Failed to listen on ${cfg.port}: ${e.message}`);
    throw e;
  });

  // Merge rather than replace: the install recorded which scope this is and who
  // it runs as, and a restart must not erase that.
  writeServiceConfig({ ...(readServiceConfig() || {}), ...cfg, url, pid: process.pid, startedAt: new Date().toISOString() });
  log(`JustImagine service listening on ${url} — gallery ${cfg.root} (${videoModels.length} video models)`);

  // A background service must not die on a stray upstream rejection.
  process.on('unhandledRejection', (e) => log(`unhandled rejection: ${e?.message || e}`));
  process.on('uncaughtException', (e) => log(`uncaught exception: ${e?.stack || e}`));

  return waitForSignals(server, () => {
    // Clear the recorded pid so `service status` doesn't claim a dead process.
    writeServiceConfig({ ...readServiceConfig(), pid: null });
    log('Stopping.');
  });
}

async function reportStatus(flags) {
  const cfg = serviceSettings(flags);
  const status = serviceStatus();
  const live = await probe(cfg.port);
  const mark = (ok) => (ok ? '\x1b[32m●\x1b[0m' : '\x1b[2m○\x1b[0m');
  console.log(logo());
  console.log(`   ${mark(status.installed)} installed   \x1b[2m${status.detail}\x1b[0m`);
  console.log(`   ${mark(!!live)} reachable   \x1b[2m${live ? `http://127.0.0.1:${cfg.port}` : `nothing answering on port ${cfg.port}`}\x1b[0m`);
  if (status.installed) {
    console.log(
      `   Scope:    ${status.scope === 'system' ? 'system — starts with the machine' : 'this user — starts when you log on'}`
    );
  }
  console.log(`   Gallery:  ${live?.root || cfg.root}`);
  console.log(`   Logs:     ${SERVICE_LOG}`);
  if (!status.installed) console.log(`\n   \x1b[2mbro imagine service install\x1b[0m  to start it at every login.`);
  else if (status.scope !== 'system') console.log(`\n   \x1b[2m${elevationHint()}\x1b[0m`);
  return live || status.installed ? 0 : 1;
}

function tailLog(lines = 40) {
  let any = false;
  for (const file of [SERVICE_LOG, SERVICE_ERR]) {
    let text = '';
    try {
      text = fs.readFileSync(file, 'utf8');
    } catch {
      continue;
    }
    if (!text.trim()) continue;
    any = true;
    console.log(`\x1b[2m── ${file}\x1b[0m`);
    console.log(text.trim().split('\n').slice(-lines).join('\n'));
  }
  if (!any) console.log(`No service log yet (${SERVICE_LOG}).`);
  return 0;
}

export async function runServiceCommand(argv) {
  const flags = parseServiceArgs(argv);
  const cmd = flags._[0] || 'status';

  if (cmd === 'help' || cmd === '-h' || cmd === '--help') {
    console.log(SERVICE_HELP);
    return 0;
  }
  if (cmd === 'run') return runService(flags);
  if (cmd === 'status') return reportStatus(flags);
  if (cmd === 'logs' || cmd === 'log') return tailLog(flags.lines || 40);

  if (cmd === 'install') {
    const previous = readServiceConfig();
    const cfg = serviceSettings(flags, { fresh: true });
    fs.mkdirSync(cfg.root, { recursive: true });
    const r = serviceInstall(cfg);
    if (!r.ok) {
      console.error(`Install failed: ${r.message || 'unknown error'}`);
      return 1;
    }
    console.log(logo());

    // Unelevated, over an install that already starts at boot: the better one
    // stays, and saying so beats silently installing a second server that could
    // never get the port.
    if (r.skipped) {
      console.log(`   \x1b[33m○ nothing changed\x1b[0m  ${r.message}`);
      console.log(`   URL:        \x1b[36mhttp://127.0.0.1:${cfg.port}\x1b[0m`);
      console.log(`\n   \x1b[2m${elevationHint()}\x1b[0m\n`);
      return 0;
    }

    const moved = previous?.root && path.resolve(previous.root) !== cfg.root;
    console.log(`   Installed:  ${r.mechanism}`);
    console.log(`   Starts:     ${r.boot ? 'with the machine, before anyone logs on' : 'when you log on'}`);
    console.log(`   Gallery:    ${cfg.root}${moved ? `  \x1b[2m(was ${previous.root})\x1b[0m` : ''}`);
    console.log(`   URL:        \x1b[36mhttp://127.0.0.1:${cfg.port}\x1b[0m`);
    if (r.replaced) console.log(`   \x1b[2mReplaced the per-user install — an elevated one takes priority.\x1b[0m`);
    const started = serviceStart();
    if (!started.ok) console.log(`   \x1b[2mCould not start it right now: ${started.message}\x1b[0m`);
    // Task Scheduler, launchd and systemd all return before the process has
    // bound the port, so confirm by asking the server itself.
    const live = await waitForPort(cfg.port, 15000);
    console.log(`   ${live ? '\x1b[32m● running\x1b[0m' : '\x1b[33m○ not answering yet — check `bro imagine service logs`\x1b[0m'}`);
    if (!r.elevated) {
      console.log(`\n   \x1b[33m⚠ Not elevated, so this runs only while you are logged on.\x1b[0m`);
      console.log(`     \x1b[2m${elevationHint()} Re-installing elevated replaces this one automatically.\x1b[0m`);
    }
    console.log(`\n   \x1b[2m'bro imagine service uninstall' removes it; your gallery is untouched.\x1b[0m\n`);
    if (live && flags.open !== false) openBrowser(`http://127.0.0.1:${cfg.port}`);
    return live ? 0 : 1;
  }

  if (cmd === 'uninstall' || cmd === 'remove') {
    const before = serviceStatus();
    serviceStop();
    const r = serviceUninstall();
    if (r.ok) {
      console.log('JustImagine service removed. Your gallery folder is untouched.');
      return 0;
    }
    console.error(`Uninstall failed: ${r.message}`);
    if (before.scope === 'system' && !isElevated()) {
      console.error(
        process.platform === 'win32'
          ? 'That one was installed for the whole machine — re-run this in an Administrator terminal.'
          : 'That one was installed for the whole machine — re-run it with sudo.'
      );
    }
    return 1;
  }

  if (cmd === 'start' || cmd === 'restart') {
    if (cmd === 'restart') serviceStop();
    const r = serviceStart();
    if (!r.ok) {
      console.error(`Start failed: ${r.message || 'unknown error'}`);
      return 1;
    }
    const cfg = serviceSettings(flags);
    const live = await waitForPort(cfg.port, 15000);
    console.log(live ? `JustImagine is running at http://127.0.0.1:${cfg.port}` : 'Started, but nothing is answering yet — try `bro imagine service logs`.');
    return live ? 0 : 1;
  }

  if (cmd === 'stop') {
    const r = serviceStop();
    console.log(r.ok ? 'JustImagine service stopped.' : `Stop failed: ${r.message}`);
    return r.ok ? 0 : 1;
  }

  console.error(`Unknown service command: ${cmd}`);
  console.error(SERVICE_HELP);
  return 1;
}

async function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const live = await probe(port, 800);
    if (live) return live;
    if (Date.now() > deadline) return null;
    await new Promise((r) => setTimeout(r, 500));
  }
}

// ---------- `bro imagine skill` ----------

// The agent-facing half of JustImagine: the HTTP API is the capability, and this
// skill is the instruction sheet for it. Shipped inside the package, copied into
// a project (or a home directory) so Claude Code and friends can find it.
export const SKILL_ID = 'generate-images-videos';
export const skillSource = () =>
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'skills', SKILL_ID);

export function skillTarget({ global: g = false, dir = '' } = {}) {
  if (dir) return path.resolve(dir);
  return path.join(g ? os.homedir() : process.cwd(), '.claude', 'skills', SKILL_ID);
}

function installSkill(flags = {}) {
  const src = skillSource();
  if (!fs.existsSync(src)) {
    console.error(`The ${SKILL_ID} skill is missing from this install (expected ${src}).`);
    return 1;
  }
  const target = skillTarget(flags);
  const existed = fs.existsSync(target);
  fs.mkdirSync(path.dirname(target), { recursive: true });
  // Overwrite rather than merge: the skill is ours, and a half-updated one is
  // worse than either version.
  fs.rmSync(target, { recursive: true, force: true });
  fs.cpSync(src, target, { recursive: true });
  console.log(`${existed ? 'Updated' : 'Installed'} the ${SKILL_ID} skill:`);
  console.log(`  ${path.join(target, 'SKILL.md')}`);
  console.log(`\nIt teaches an agent to batch-generate images and video through the`);
  console.log(`gallery's API. ${flags.global ? 'Available in every project.' : 'Available in this project; --global installs it for every project.'}`);
  return 0;
}

// ---------- `bro imagine open` ----------

async function openRunning(flags) {
  const cfg = serviceSettings(flags);
  const live = await probe(cfg.port);
  if (live) {
    const url = `http://127.0.0.1:${cfg.port}`;
    // --no-open makes this a "where is it running?" query for scripts.
    console.log(flags.open === false ? `${url}  \x1b[2m(${live.root})\x1b[0m` : `Opening ${url}  \x1b[2m(${live.root})\x1b[0m`);
    if (flags.open !== false) openBrowser(url);
    return 0;
  }
  console.error(`Nothing is running on port ${cfg.port}.`);
  console.error('  bro imagine service install   start it in the background, at every login');
  console.error('  bro imagine                   run it in this terminal instead');
  return 1;
}

// ---------- command dispatch ----------

// Everything under `bro imagine …`. Returns null when the args are not a
// sub-command, so the caller falls through to opening the gallery.
export async function runImagineCommand(argv, { config } = {}) {
  const [cmd, ...rest] = argv;
  if (cmd === 'service' || cmd === 'daemon') return runServiceCommand(rest);
  if (cmd === 'open') return openRunning(parseServiceArgs(rest));
  if (cmd === 'skill' || cmd === 'skills') return installSkill(parseServiceArgs(rest));
  if (cmd === 'help' || cmd === '-h' || cmd === '--help') {
    console.log(imagineHelp(config || loadConfig()));
    return 0;
  }
  return null;
}

// Flags for a bare `bro imagine`.
export function parseImagineArgs(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--root') out.root = argv[++i];
    else if (a === '--port') out.port = Number(argv[++i]);
    else if (a === '--no-open') out.open = false;
    else if (a === '-p' || a === '--provider' || a === '--api') out.api = argv[++i];
    else out._.push(a);
  }
  return out;
}

