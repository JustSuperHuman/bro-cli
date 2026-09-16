# JustImagine development

The canonical app lives in the private [TekPrepperAI repository](https://github.com/JustSuperHuman/TekPrepperAI), pinned at `vendor/justimagine` as a Git submodule.

After cloning Bro, initialize it with:

```sh
git submodule update --init --recursive
```

You need access to that private repository for source development. Published Bro packages include the shared app and required assets, so installed CLI users do not need GitHub access.

Make changes to the app's server, UI, generation and storage in TekPrepperAI, publish them there, then update and commit the submodule pointer here. `src/justimagine.js` is the Bro adapter: it supplies Bro's keys, paths, terminal UI, model catalog and OS service integration, with authentication disabled for the local gallery.

The hosted suite has a separate account server. Bro packaging includes shared source and assets only; it excludes `suite/` and hosted account administration. Keep hosted-only tools outside the shared packaging allowlist.

Run the focused integration checks from Bro:

```sh
bun test ./src/justimagine.test.js ./src/justimagine-service.test.js ./src/justimagine-server.test.js ./src/justimagine-gen.test.js ./src/justimagine-store.test.js ./src/justimagine-characters.test.js ./src/justimagine-ui.test.js
```
