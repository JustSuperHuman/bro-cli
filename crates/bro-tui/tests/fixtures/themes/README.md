These fixtures contain six published VS Code palettes and native T3 Code test data. Each source directory includes its complete MIT license notice. Keep those notices with the data when redistributing it.

The six VS Code files retain their original bytes, including comments, whitespace, and line endings. The local `.gitattributes` disables newline conversion. The native JSON files are documented derivatives of upstream data. No upstream build scripts or extension theme generators were run.

## Palette sources

| Local file | Pinned source | Original location |
| --- | --- | --- |
| `catppuccin/mocha.json` | [catppuccin/vscode, `befc9e6fc41980f4241408f7049755d47c06ff45`](https://github.com/catppuccin/vscode/tree/befc9e6fc41980f4241408f7049755d47c06ff45) | `extension/themes/mocha.json` inside the [3.19.0 VSIX release](https://github.com/catppuccin/vscode/releases/tag/catppuccin-vsc-v3.19.0), asset ID `410449961` |
| `dracula/dracula.json` | [dracula/visual-studio-code, `9726e1f6b51a4382334bc36e03128fcf02c3fb45`](https://github.com/dracula/visual-studio-code/tree/9726e1f6b51a4382334bc36e03128fcf02c3fb45) | `extension/theme/dracula.json` inside the [2.24.3 VSIX release](https://github.com/dracula/visual-studio-code/releases/tag/v2.24.3), asset ID `117170259` |
| `nord/nord-color-theme.json` | [nordtheme/visual-studio-code, `8ead09822c02d0d49d0f764104505e5a34d3689f`](https://github.com/nordtheme/visual-studio-code/blob/8ead09822c02d0d49d0f764104505e5a34d3689f/themes/nord-color-theme.json) | `themes/nord-color-theme.json` |
| `one-dark-pro/OneDark-Pro.json` | [Binaryify/OneDark-Pro, `75c80ab18d90655e05ad837bb8d89f5e68d5ce53`](https://github.com/Binaryify/OneDark-Pro/blob/75c80ab18d90655e05ad837bb8d89f5e68d5ce53/themes/OneDark-Pro.json) | `themes/OneDark-Pro.json` |
| `vitesse/vitesse-dark.json` | [antfu/vscode-theme-vitesse, `2862595c3d5d05fabfd5aeb50bcbb79ea2f8d85f`](https://github.com/antfu/vscode-theme-vitesse/blob/2862595c3d5d05fabfd5aeb50bcbb79ea2f8d85f/themes/vitesse-dark.json) | `themes/vitesse-dark.json` |
| `vscode/solarized-light-color-theme.json` | [microsoft/vscode, `73dd5a077780bdd3bba09fbeac0abca62d1244f9`](https://github.com/microsoft/vscode/blob/73dd5a077780bdd3bba09fbeac0abca62d1244f9/extensions/theme-solarized-light/themes/solarized-light-color-theme.json) | `extensions/theme-solarized-light/themes/solarized-light-color-theme.json` |

Catppuccin and Dracula's notices come from `extension/LICENSE.txt` in their respective VSIX files. Nord's notice comes from the pinned repository's `license`; One Dark Pro and VS Code use `LICENSE.txt`; Vitesse uses `LICENSE.md`.

## Native T3 data

The native sources are pinned to [pingdotgg/t3code `5a20d59db38abef8e0b98cfe47fed81d074bcecc`](https://github.com/pingdotgg/t3code/tree/5a20d59db38abef8e0b98cfe47fed81d074bcecc), under the MIT notice in `t3code/LICENSE`.

`t3code/t3-chat-export.json` is a `serializeThemeFile`-compatible representation of [the actual `T3_CHAT_THEME`](https://github.com/pingdotgg/t3code/blob/5a20d59db38abef8e0b98cfe47fed81d074bcecc/packages/shared/src/themePalettes.ts#L238-L371). Its full light and dark role maps are unchanged. The ID is `t3-chat-fixture` and the name is `T3 Chat fixture`, because native imports reject the reserved `t3-chat` ID. The built-in `sidebarArtwork` flag is omitted, as it is by upstream export.

The same role maps are in `crates/bro-tui/assets/themes/t3-default-light.json` and `t3-default-dark.json` for runtime fallback. Every role and value was compared with the pinned TypeScript source. Each map has 61 roles. Upstream `getDefaultThemeColors` uses these T3 Chat maps when a native file omits roles.

`t3code/literal-colors.json` copies the input object from the upstream [`themePalette.test.ts`](https://github.com/pingdotgg/t3code/blob/5a20d59db38abef8e0b98cfe47fed81d074bcecc/apps/web/src/themePalette.test.ts) test named `decodes literal CSS color formats into OKLCH without dropping alpha`. Only the TypeScript object notation and whitespace were converted to JSON. It covers OKLCH percentages, alpha hex, RGB, HSL, HWB, Lab, LCH, OKLab, display-p3, named colors, `transparent`, and `none` alpha.

`t3code/vscode-expected.json` contains all 61 rendered sRGB role colors for each of the six palettes, computed
with the pinned upstream `parseThemeImport` / `parseVsCodeThemeFile`, native parser, palette derivation and
`themeColorToHex` functions. The isolated reference used Culori 4.0.2 and jsonc-parser 3.3.1. Only inspected,
pure functions were extracted; browser storage, application startup and extension code were not loaded. These
snapshots keep the regression suite offline and independent of Bro's converter.

## Regression cases

Catppuccin has a fully transparent input border and an opaque input background. Vitesse has a fully transparent focus border, translucent foregrounds, and no top-level appearance type. Nord has a translucent button background and requires JSONC support for its line comments. Dracula also omits the appearance type. Solarized Light requires JSONC support too: its original file has line comments and trailing commas. All six have large syntax-token sections that the upstream T3 importer ignores when deriving GUI colors.

Native files require strict JSON, a version of 1, and at least one supported role in each supplied color map. Their variants contain role maps directly; they do not wrap another `colors` object. The opposite appearance fills missing roles from its own defaults. CSS is an optional JSON string; it is not a standalone file-import format.

## SHA-256

The following checksums cover the original palette file bytes:

```text
ae9a2cd5944a185e5f45c8ff1408342b3ae5ebc317c8238db6fe5fb0402e3e48  catppuccin/mocha.json
ae473d11bdef6ad5d3fc3c6cf3a104e98c9fd8a195f13c5cc1699df33a2537eb  dracula/dracula.json
cd15e5a0c2edb2e373196ed2ab0f0d90c0eccc61f82c37131a4efc3be2ca8061  nord/nord-color-theme.json
bd098a63d2fdcb932ea67caaf66eb25a51cda1c2179ff37048fe1db7fe13461f  one-dark-pro/OneDark-Pro.json
4c0d1ea6503bf17599fe1d615232ebae972028b93470f95431e8b61cc647e4f4  vitesse/vitesse-dark.json
7c10e85f312c79dfccfa97fe97bb74c3d716a35f55e9701b30bc57e725ce1b44  vscode/solarized-light-color-theme.json
```

The complete downloaded VSIX files, which are not checked in, had these SHA-256 values:

```text
ebf347664837edbe91c9920ff3d14c96d4a28beeec0b95137c76058326329780  catppuccin-vsc-3.19.0.vsix
03f3b92496dd7f9983bbeed0864681e049d7a94e459430b299f47b3e2e8e3a75  dracula.vsix
```

GitHub published Catppuccin's matching `sha256:` asset digest. Dracula's release metadata has no asset digest; the value above was calculated locally from asset `117170259`.
