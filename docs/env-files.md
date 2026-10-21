# Env files

```
bro --export-env                     # your variables → ./bro.env, encrypted (asks for a passphrase)
bro --env bro.env                    # open bro with them; every session it starts inherits them
bro --env https://m.justgains.com    # a folder URL loads bro.env from there (then bro.env.enc)
bro --env bro.env --save             # keep them for every new program and service
bro env encrypt keys.env             # keys.env → keys.env.enc
bro env list bro.env                 # names and lengths, values hidden
bro env decrypt bro.env -o keys.env
```

An env file is plain `KEY=value` lines (`export`, quotes and `#` comments work), so one file can
carry `OPENROUTER_API_KEY`, `ELEVENLABS_API_KEY` and whatever else a new machine needs. Encrypted
files use Argon2id and XChaCha20-Poly1305 and ask for their passphrase when loaded, or read it from
`BRO_ENV_PASSWORD`. URLs must be https; plain http only works for localhost. bro ignores PATH, HOME
and the other variables the OS sets.

`--export-env` reads your user environment variables on Windows (`HKCU\Environment`) and the
current shell's environment on macOS and Linux. It leaves out OS, login and terminal variables
unless you pass `--all`. `--plain` skips the encryption, `-` as the file name prints it, and
`--force` replaces an existing file.

`--save` writes to `HKCU\Environment` on Windows. On macOS and Linux it writes
`~/.config/bro/env` (readable only by you), sourced from `~/.profile`, `~/.bashrc` and `~/.zshenv`,
plus fish's `conf.d`. It also runs `launchctl setenv` on macOS, and on Linux it writes systemd's
`environment.d` and imports the variables into the running user manager. New terminals and services
get the variables; ones already running keep what they had. If JustImagine is installed, its service
restarts to pick up the keys.
