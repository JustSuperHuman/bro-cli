#!/usr/bin/env node
import { main } from '../src/cli.js';

main(process.argv.slice(2))
  .then((code) => process.exit(typeof code === 'number' ? code : 0))
  .catch((err) => {
    console.error(err?.message || err);
    process.exit(1);
  });
