// Compatibility facade: JustImagine lives in the vendor submodule.
import path from 'node:path';
import { BRO_DIR } from './config.js';
import { ensureLibrary as ensure, listCharacters as list } from '../vendor/justimagine/src/justimagine-characters.js';
export * from '../vendor/justimagine/src/justimagine-characters.js';
export const CHARACTERS_DIR = path.join(BRO_DIR, 'justimagine', 'characters');
export const ensureLibrary = (root = CHARACTERS_DIR) => ensure(root);
export const listCharacters = (root = CHARACTERS_DIR) => list(root);
