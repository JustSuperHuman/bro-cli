Connect this computer to your Just Superhuman account with `bro jsh login`, then `bro computers enable`. You can also switch on "Reach this computer from your account" under Settings → Remote Control → JustConnect. Enabling installs the RMM agent with OS administrator permission. Your account owns the connection; you, account admins and IT Admins can access it.

Everything Bro does on another computer goes through that account connection. There is no address to type, no port to open and no key to copy: a computer is reachable when it is on your account, its access is on and Bro is running there.

## The Computers view

Open it from the palette (search "computers"). It lists every computer on your account, this one first, with a dot for whether each can run sessions right now and a few words on where it stands ("ready · 3 sessions", "closed · last open 2 hr ago", "access turned off").

Pick one to see more:

- A computer that is ready shows its sessions, the ones waiting on someone first, with what each is doing and how long a request there and back took.
- A computer that isn't shows the four links between here and it (this computer, Just Superhuman, that computer, Bro there), which of them answered, and what fixes the first one that didn't.
- **Open desktop** opens its screen in your browser. **Run bro ctl there** points `bro ctl` at it; the same button brings it back. **Install Bro there** or **Update Bro there** appears for a Windows computer that has no Bro or an older one.

The list is kept while Bro looks again, and when a look fails: a notice above it says the account couldn't be reached, and the header says how old the list is.

Under Settings → Remote Control, the JustConnect row also says where this computer's own connection stands: connected (and whether work starts at once or within a few seconds), reconnecting with the time of the next try, or refused.

## From your phone

Sign in to the bro app with the same account (Continue with Just Superhuman, then approve the phone on your account's page). It lists every computer on your accounts. A computer with Bro open goes straight to its sessions, the same as on your own Wi-Fi: chats, terminals, files, voice and Take Control. Nothing on the computer listens on the internet. When you open it, Bro there dials out to Just Superhuman and the phone's requests travel down that connection, which closes again two minutes after you leave.

A computer with Bro closed, or with no Bro at all, still has a page in the app with its desktop and a command line. Both go through the monitoring service, so they work at the sign-in screen too. Commands there run as the system account.

Who can open a computer from a phone is who can reach it anywhere else on the account: you, account admins and IT Admins. Just Superhuman staff can open any account's computers to support it. The Remote Control page names whoever is connected ("James Greene, through your account", with "Just Superhuman support" added for staff), and turning off "Reach this computer from your account" disconnects them at once. A phone that comes in through the account is never treated as this computer: it can't change the account Bro is signed in to or turn JustConnect on or off here.

Bro older than 2.1.58 can't be opened this way. The app says so, and the computer's page still offers its desktop and command line.

## From a terminal

From another signed-in computer, use `bro computers list` and `bro computers select <id>`. Your subsequent `bro ctl` calls run on that computer. For example:

```text
bro ctl open_session project=C:/work/app agent=codex task="Run the tests"
bro computers status <id>
bro computers doctor <id>
bro computers run <id> powershell "hostname"
bro computers desktop <id>
bro computers history <id>
bro computers select local
```

Each `bro computers` command answers in plain text. Add `--json` for the server's raw answer in a script.

`bro computers doctor <id>` checks each link in turn and prints what it found, what to do about the first that fails, and the commands that do it. When every link answers it sends one small request and reports how long it took there and back. `bro computers status` (no ID) says whether this computer's Bro is connected and how quickly work reaches it.

A connection that fails is explained the same way wherever it shows up (`bro computers`, a remote `bro ctl`, the view): signed out, no way out to the internet, the account not answering, access off, the computer switched off, Bro closed there. Work that didn't finish says what became of it: the tool's own reason when it ran and failed, "never delivered" when Bro there didn't pick it up, and a warning to check before repeating when it was handed over and no result came back.

Managed worker sessions retain their scoped local `bro ctl`. The orchestrator can explicitly call `list_computers` and `run_on_computer`. Remote Bro tools are limited to listing projects/sessions, opening a session, reading sessions/chats, sending input and closing sessions.

## How work reaches a computer

Keep Bro open on the target for those jobs. Its worker makes outbound HTTPS requests with the current device sign-in. With phone access off, the worker's bridge listens only on loopback. Commands through the RMM service can run while Bro is closed and use the service's system permissions.

The worker asks the account server for work and is kept waiting until some arrives, so a request made elsewhere starts here about a round trip later. The same answer says when a phone wants this computer, and the worker then opens the connection the phone is carried over. An account server that answers at once instead is asked on a clock: every quarter second for ten seconds after work arrived, slowing to every five seconds when the computer has been left alone for five minutes. Whoever asked follows the job the same way, hearing the moment it settles.

Up to four jobs run at once. Lists and reads never wait behind a slow start (opening a session that first installs its agent, say). What changes a session (typing into it, pressing keys, answering a prompt, closing it) runs in the order it was asked, one at a time per session.

When the account server can't be reached, the worker stays away a little longer after each try, up to a minute, with some chance in each wait so an account's computers don't all return in the same instant. Turning access off, or signing out, hangs up a request the server is holding, so no work is handed to a computer that won't run it.

A request that got no answer is repeated with the same request ID, which the server answers with the job it already made: the work runs once.

`bro computers disable` stops new access and queued work but leaves the monitoring agent installed. Signing out stops the local Bro worker and clears computer selection; disable the computer in JustConnect to stop account access through RMM too. If a job is `unknown`, check its history before submitting it again: delivery may have succeeded without a response.

Deployment settings, the API contract, platform requirements and the account transfer procedure are in the Tactical RMM repository's `docs/justconnect.md`. A live pilot is required to verify the deployed installer and MeshCentral configuration.
