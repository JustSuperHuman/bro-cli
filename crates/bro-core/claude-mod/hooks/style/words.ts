// The spinner's word and the turn line's past tense, in the voice of the bro
// theme on screen. Each family pairs a word with its past form, so "Forging…"
// while Claude works reads "Forged for 12s" when the turn ends.

export type Family = {
  id: string
  pairs: readonly (readonly [now: string, past: string])[]
}

const STUDIO: Family = {
  id: 'studio',
  pairs: [
    ['Drafting', 'Drafted'],
    ['Sketching', 'Sketched'],
    ['Composing', 'Composed'],
    ['Refining', 'Refined'],
    ['Polishing', 'Polished'],
    ['Assembling', 'Assembled'],
    ['Calibrating', 'Calibrated'],
    ['Tuning', 'Tuned'],
    ['Shaping', 'Shaped'],
    ['Machining', 'Machined'],
    ['Proofing', 'Proofed'],
    ['Honing', 'Honed'],
  ],
}

const FAMILIES: readonly (Family & { themes: readonly string[] })[] = [
  {
    id: 'neon',
    themes: ['synthwave', 'synthwave-84', 'outrun', 'vaporwave', 'ultra', 'panda'],
    pairs: [
      ['Overclocking', 'Overclocked'],
      ['Synthesizing', 'Synthesized'],
      ['Arpeggiating', 'Arpeggiated'],
      ['Laser-etching', 'Laser-etched'],
      ['Neon-wiring', 'Neon-wired'],
      ['Chroming', 'Chromed'],
      ['Wavebending', 'Wavebent'],
      ['Remixing', 'Remixed'],
      ['Turbo-boosting', 'Turbo-boosted'],
      ['Pixel-pushing', 'Pixel-pushed'],
      ['Moonwalking', 'Moonwalked'],
      ['Afterburning', 'Afterburned'],
    ],
  },
  {
    id: 'terminal',
    themes: ['matrix', 'digital-rain', 'phosphor', 'terminal', 'cyber', 'amber'],
    pairs: [
      ['Compiling', 'Compiled'],
      ['Decrypting', 'Decrypted'],
      ['Tracing', 'Traced'],
      ['Piping', 'Piped'],
      ['Grepping', 'Grepped'],
      ['Forking', 'Forked'],
      ['Patching', 'Patched'],
      ['Linking', 'Linked'],
      ['Spooling', 'Spooled'],
      ['Parsing', 'Parsed'],
      ['Jacking in', 'Jacked in'],
      ['Bootstrapping', 'Bootstrapped'],
    ],
  },
  {
    id: 'forge',
    themes: ['ember', 'gruvbox', 'monokai-pro', 'ayu-dark', 'ayu-mirage', 'ayu-light', 'oriel'],
    pairs: [
      ['Forging', 'Forged'],
      ['Smelting', 'Smelted'],
      ['Tempering', 'Tempered'],
      ['Kindling', 'Kindled'],
      ['Stoking', 'Stoked'],
      ['Annealing', 'Annealed'],
      ['Hammering', 'Hammered'],
      ['Quenching', 'Quenched'],
      ['Welding', 'Welded'],
      ['Riveting', 'Riveted'],
      ['Casting', 'Cast'],
      ['Firing', 'Fired'],
    ],
  },
  {
    id: 'tide',
    themes: ['ocean', 'cobalt2', 'noctis', 'solarized-dark', 'solarized-light', 'night-owl-light'],
    pairs: [
      ['Charting', 'Charted'],
      ['Sounding', 'Sounded'],
      ['Navigating', 'Navigated'],
      ['Surfacing', 'Surfaced'],
      ['Drifting', 'Drifted'],
      ['Trawling', 'Trawled'],
      ['Sailing', 'Sailed'],
      ['Anchoring', 'Anchored'],
      ['Tacking', 'Tacked'],
      ['Beachcombing', 'Beachcombed'],
      ['Snorkeling', 'Snorkeled'],
      ['Diving', 'Dived'],
    ],
  },
  {
    id: 'night',
    themes: ['tokyo-night', 'tokyo-night-storm', 'night-owl', 'palenight', 'poimandres', 'nebula'],
    pairs: [
      ['Moonlighting', 'Moonlighted'],
      ['Stargazing', 'Stargazed'],
      ['Lantern-lighting', 'Lantern-lit'],
      ['Dreaming', 'Dreamt'],
      ['Wandering', 'Wandered'],
      ['Starhopping', 'Starhopped'],
      ['Skywatching', 'Skywatched'],
      ['Constellating', 'Constellated'],
      ['Night-owling', 'Night-owled'],
      ['Orbiting', 'Orbited'],
      ['Midnight-oiling', 'Midnight-oiled'],
      ['Comet-chasing', 'Comet-chased'],
    ],
  },
  {
    id: 'grove',
    themes: ['forest', 'everforest', 'kanagawa', 'vitesse-dark'],
    pairs: [
      ['Growing', 'Grew'],
      ['Grafting', 'Grafted'],
      ['Pruning', 'Pruned'],
      ['Cultivating', 'Cultivated'],
      ['Rooting', 'Rooted'],
      ['Sprouting', 'Sprouted'],
      ['Foraging', 'Foraged'],
      ['Weaving', 'Wove'],
      ['Mulching', 'Mulched'],
      ['Harvesting', 'Harvested'],
      ['Tending', 'Tended'],
      ['Branching', 'Branched'],
    ],
  },
  {
    id: 'bloom',
    themes: [
      'sakura',
      'rose-pine',
      'rose-pine-dawn',
      'catppuccin-mocha',
      'catppuccin-macchiato',
      'catppuccin-frappe',
      'catppuccin-latte',
    ],
    pairs: [
      ['Blooming', 'Bloomed'],
      ['Arranging', 'Arranged'],
      ['Brewing', 'Brewed'],
      ['Steeping', 'Steeped'],
      ['Whisking', 'Whisked'],
      ['Folding', 'Folded'],
      ['Embroidering', 'Embroidered'],
      ['Glazing', 'Glazed'],
      ['Frosting', 'Frosted'],
      ['Daydreaming', 'Daydreamed'],
      ['Petal-pressing', 'Petal-pressed'],
      ['Icing', 'Iced'],
    ],
  },
  {
    id: 'arcane',
    themes: ['dracula', 'aura', 'shades-of-purple', 'andromeda'],
    pairs: [
      ['Conjuring', 'Conjured'],
      ['Summoning', 'Summoned'],
      ['Enchanting', 'Enchanted'],
      ['Bewitching', 'Bewitched'],
      ['Divining', 'Divined'],
      ['Scrying', 'Scried'],
      ['Hexing', 'Hexed'],
      ['Transmuting', 'Transmuted'],
      ['Invoking', 'Invoked'],
      ['Channeling', 'Channeled'],
      ['Spellcasting', 'Spellcast'],
      ['Potion-brewing', 'Potion-brewed'],
    ],
  },
  {
    id: 'frost',
    themes: ['nord'],
    pairs: [
      ['Chiseling', 'Chiseled'],
      ['Crystallizing', 'Crystallized'],
      ['Ice-carving', 'Ice-carved'],
      ['Sledding', 'Sledded'],
      ['Snowshoeing', 'Snowshoed'],
      ['Glinting', 'Glinted'],
      ['Hibernating', 'Hibernated'],
      ['Frosting', 'Frosted'],
      ['Fjord-mapping', 'Fjord-mapped'],
      ['Aurora-chasing', 'Aurora-chased'],
    ],
  },
]

/** The family for a theme: its own, its base's (a custom theme), else the neutral one. */
export function familyFor(id: string, base: string): Family {
  return FAMILIES.find(f => f.themes.includes(id)) ?? FAMILIES.find(f => f.themes.includes(base)) ?? STUDIO
}

/** Every family, for the tests. */
export const ALL_FAMILIES: readonly Family[] = [STUDIO, ...FAMILIES]

/** A stable pick: the same seed always lands on the same pair. */
export function pick(family: Family, seed: string): readonly [string, string] {
  let hash = 0x811c9dc5
  for (let i = 0; i < seed.length; i += 1) {
    hash ^= seed.charCodeAt(i)
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return family.pairs[hash % family.pairs.length] ?? family.pairs[0] ?? ['Working', 'Worked']
}
