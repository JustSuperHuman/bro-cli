`t3-default-light.json` and `t3-default-dark.json` are the complete T3 Chat fallback palettes from
pingdotgg/t3code commit `5a20d59db38abef8e0b98cfe47fed81d074bcecc`. The MIT notice is in `LICENSE-t3code.txt`.

`aurora-studio.json` is Bro's paired light/dark demonstration, generated with the ported T3 vivid-palette engine
and adjusted for violet/sea-glass surfaces, messages and code. Its embedded stylesheet is also kept in
`aurora-studio.css`; the standalone CSS is a source asset, not an additional import format.

The stylesheet uses T3's documented hooks from `packages/shared/src/themeCss.ts` at
`02f8a3280b8635d75a64b21587aef678b4391b51`. It contains no external resource loads or animations. It is active-theme-only
and uses appearance classes rather than a theme ID, so T3's live-preview ID does not disable it.

Bro recognizes this exact bundled stylesheet and supplies a native equivalent for ambient gradients and rounded
message/code borders. That rendering remains palette-driven when the theme is copied or edited. This is a
specific native treatment, not a general CSS engine; other CSS is kept as export data and is not executed.
