#!/usr/bin/env node
import { main } from '../src/cli.js';

// Set process.exitCode and let Node exit when the event loop drains, rather than
// forcing process.exit(). A hard exit can race with in-flight fetch/undici socket
// teardown and trip a libuv assertion on Windows (src/win/async.c).
main(process.argv.slice(2))
  .then((code) => {
    process.exitCode = typeof code === 'number' ? code : 0;
  })
  .catch((err) => {
    console.error(err?.message || err);
    process.exitCode = 1;
  });
