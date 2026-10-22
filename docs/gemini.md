# Gemini on your Google AI subscription

```
bro gemini login     # sign in with Google in the browser
bro gemini status    # account, plan, and quota left per model
bro gemini logout
```

The `gemini` provider runs Claude Code, Codex, Pi and omp on Gemini 4 Argon, billed to your Google AI
Pro or Ultra plan instead of an API key. Pick **gemini** in New session (`alt+n`); if you haven't signed
in yet, Enter opens the sign-in in a tab. bro-proxy translates each agent's requests into Gemini's
format and back, tools and thinking included.

`gemini-4-argon` follows the agent's own effort setting (low, medium or high thinking).
`gemini-4-argon-high` and `gemini-4-argon-low` pin a level.

bro signs in the way Google's Antigravity CLI does, with Antigravity's OAuth client, and talks to the same
Cloud Code Assist service. To use a different Google client, set both `BRO_GEMINI_CLIENT_ID` and
`BRO_GEMINI_CLIENT_SECRET` (in your environment, or an env file loaded with `bro --env`, see
[env files](env-files.md)). A sign-in only refreshes with the client it was made with, so run
`bro gemini login` again after switching.
Google's terms only allow that sign-in in Google's own apps. In February 2026 Google suspended
Antigravity and Gemini CLI access for paying users who used it from other tools. Use an account you
can afford to lose that access on. The sign-in is saved in `~/.bro/gemini/oauth.json`.
