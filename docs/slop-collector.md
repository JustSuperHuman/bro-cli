# Slop Collector (beta)

Renders and tests your agents run can leave things behind when they get cut short by a tool timeout, Ctrl+C or an
agent giving up:

- **Programs left running.** Remotion, Playwright and Puppeteer start a browser with no window (a headless browser)
  to do their work. When the script driving it dies, the browser keeps running with nothing left to use it, holding
  its memory until the session closes. A session can stay open for days.
- **Temp files.** The same tools make a throwaway browser profile or render folder in your temp folder for every run
  and delete it when the run ends normally. A run that's cut short leaves it there. They add up: on the machine
  this was built on, hundreds of them held more than 14 GB.

Slop Collector finds both and cleans them up. Turn it on under Settings › Slop Collector (off by default). It runs in
the session server, so it covers the sessions that keep running there (Settings › Session server › Keep sessions
running, on by default) whether or not a bro window is open.

Saved in `~/.bro/v2.toml`:

```toml
[slop]
on = false             # look for leftovers at all
collect = true         # clean up on its own (false: list what it finds; Clean up now cleans it up)
wait_mins = 10         # how long a leftover sits unused first: 2, 5, 10, 15, 30 or 60
anything_idle = false  # close any program left running on its own once it goes idle, not only what renders and tests leave
```

The settings page shows **Clean up** as one choice: after 2 to 60 minutes unused, or "when I say" (`collect = false`).

From a shell (or an agent): `bro slop` says what it finds and what it has cleaned up; `bro slop collect` cleans up
everything it lists now.

## Programs

Only a process whose parent has exited (**detached**), along with everything under it, can be slop. Anything still
under the session's program, or under any other live process, is in use.

- **Unattended**: a detached group led by an automated browser (headless, or driven over DevTools), one of
  Remotion's helpers (its compositor, its ffmpeg) or a browser driver (chromedriver, msedgedriver, geckodriver). Its
  script is gone, so nothing can drive it: it's unused from the moment the collector sees it.
- **Quiet**: a detached group with one of those further down (a hung script and the browser under it). While it uses
  the CPU it's working, and the page lists it under "running on their own"; once it stops (2% of one core or less),
  it's unused from then. A new process in the group counts as working.
- With `anything_idle`, every other detached group too, the same way. With it off, those are only listed, for you to
  decide.

Use starts the clock over: a window on screen, or a connection from outside the group to a port one of its processes
listens on (a DevTools client, your browser on a dev server). So "unused for 10 min" means nothing has touched it for
10 minutes, not that it was found 10 minutes ago.

### What it never closes

- anything with a window on screen
- a group with a server in it: a process other than a browser or a driver listening on a TCP port, in a group with no
  automated browser (a dev server), and Remotion Studio always. A Remotion render's own server, which only feeds the
  render's browser, doesn't count: the render is still slop once it's stuck.
- a group something outside it is connected to
- the session's own program, and anything still attached to it

A process is ended only while its session still holds it, and only if it is the same process that was seen (its
start time matches), so a reused PID is never touched. Whatever a group started since the look (a new renderer)
goes with it.

## Temp files

Only these folders, directly in the temp folder:

| Folder | Whose | Cleared when |
| --- | --- | --- |
| `puppeteer_dev_chrome_profile-*`, `puppeteer_dev_firefox_profile-*` | Puppeteer (and Remotion's Chrome) | no browser on the machine was started with it, Chrome's `lockfile` isn't in it, and it's 10 minutes old |
| `playwright_chromiumdev_profile-*`, `playwright_firefoxdev_profile-*`, `playwright_webkitdev_profile-*` | Playwright | the same |
| `remotion-webpack-bundle-*`, `remotion-v*-assets*`, `react-motion-render*` | Remotion | untouched for a day, and since before anything of Remotion's now running on the machine started (a process can't have made a folder before it existed) |
| `playwright-artifacts-*` | Playwright | empty, and untouched for a day |

The profile of a browser the collector just closed goes right away, without the 10 minutes, and counts as part of
that cleanup.

Each folder is renamed aside (`<name>.bro-slop`) before it's deleted. Windows refuses that rename while anything has
a file in the folder open or works inside it, so a folder in use is never touched, and nothing can open the folder
under its old name while it's deleted. A delete that stops partway leaves the renamed folder, which goes next time.
Links are never followed out of the temp folder.

The temp folder is listed every 10 minutes (right away when you ask, or when a browser it closed lets go of its
profile). Clearing takes up to 3 seconds per look (10 when you ask), and the server looks again within a second
until a backlog is gone.

## How it works

`crates/bro-core/src/slop.rs` holds the decisions, `slop/temp.rs` the temp folders, and `slop/windows.rs` reads the
system. Each session already runs in a Job Object ([`proc_tree`](../crates/bro-core/src/proc_tree.rs)), so the
collector lists a session's processes from its job, however they were started. Every 30 seconds while it's on, the
session server (`crates/bro-host/src/server.rs`, `slop_watch`) takes one process snapshot, reads each member's start
and CPU times, private memory and path (and command line, for browsers and script runners), groups the detached
ones, and only when there are any, looks up visible windows and TCP listeners, connections and who's on the other
end of each (loopback connections are matched to their owners).

The server sends every client its report (`SLOP` frames; `HostInfo::slop` on connect). A client asks for a look now
or for Clean up now with `SLOP_ACT`. Both are additive: an older window ignores the report, and an older server
ignores the ask, so the settings page offers a restart instead. Each leftover carries a plain name (`title`: "Chrome
from Remotion") and an exact one (`what`: "Remotion's headless Chrome"); the page shows the plain one first.
Cleanups are kept in `~/.bro/v2-slop.json` (running totals of memory and disk, and the last 25), and a window shows a
note when one happens.

`BRO_SLOP_TEMP` points the collector at another temp folder (the session server's own tests clear none).

Windows only for now. On macOS and Linux a session's processes share its process group, which a launcher like
Puppeteer leaves on purpose, so finding them there takes a different approach, and a rename there doesn't fail
while a folder is in use.
