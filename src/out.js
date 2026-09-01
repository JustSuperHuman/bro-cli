// Where bro's own words go.
//
// stdout belongs to the harness. `bro -p zai -m glm-5.3 --print "…"
// --output-format json | jq` has to see the model's JSON and nothing else, so
// a "Launching Z.ai / glm-5.3…" banner in front of it is not decoration —
// it is corruption. Progress, banners and status are diagnostics and go to
// stderr, which is where every other command-line tool puts them and which a
// terminal shows just the same.
//
// Output the user actually asked for is a result, not a diagnostic, and stays
// on stdout: --list, --dry-run, bro tokens, bro profiles, help, version.
export const note = (...args) => console.error(...args);
