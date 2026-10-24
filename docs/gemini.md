# Gemini in Bro

## Gemini API keys

Bro detects `GEMINI_API_KEY`, `GOOGLE_API_KEY`, and Gemini keys in `~/.bro/config.json`
(`keys.gemini-api`, `keys.google`, or `keys.gemini`). A detected key appears as Gemini in the sidebar's
usage panel and as `gemini:local` in Profiles. The subscription provider remains separate.

Choose **Gemini** on the Usage page or press **g** in Profiles to add a named API key profile.
Give the profile a name, then paste its key. The field masks the key. Leaving it blank uses the
detected key through a reference, so Bro does not copy an environment key to disk. Bro checks access
by listing models before saving; the check does not generate content. Named profiles can update
their key with **e** in Profiles.

Click a Gemini usage row or **Launch** on its card to choose Claude Code, Codex, Pi, or omp and a
model. Switching tools keeps the Gemini profile selected. Bro loads the available text models
from Google's API. All four tools connect through Bro's translating proxy.

The Usage card shows today's and this month's requests and reported tokens from Gemini sessions
routed through Bro on this computer. These totals survive restarts. Requests without token metadata
are identified as incomplete coverage. Bro does not infer spending, a remaining balance, or quota
percentages from those counts. **AI Studio** opens Google's account usage page for billing and limits.

## Google AI subscription

Connected subscriptions appear as `gemini-sub:local` in Profiles, the usage panel, and the launcher.
Choose **Google** on the Usage or Profiles page to connect through Bro's existing Google sign-in.
The subscription card reads live quota from Google for each Gemini model and shows its reset time.
Thinking levels share a row when Google reports the same remaining quota and reset for every level.
Different limits stay separate. A missing reading is shown as unavailable.

Click the subscription's usage row or **Launch** to choose a tool and model. The subscription remains
selected when you switch between Claude Code, Codex, Pi, and omp. The model picker uses the Gemini
models Google offers to the connected account. The API key profile's counters are separate from
the subscription's allowance.

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
