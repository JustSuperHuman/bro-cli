// Bro supplies local configuration and paths to the shared app implementation.
import { createImagineCli } from '../vendor/justimagine/src/justimagine.js';
import * as config from './config.js';
import * as catalogues from './models.js';
import * as ui from './ui.js';
import * as state from './state.js';
import * as service from './justimagine-service.js';
import { CHARACTERS_DIR } from './justimagine-characters.js';
export { IMAGE_APIS, mergeImageApis } from '../vendor/justimagine/src/justimagine.js';
export const {
  IMAGINE_PROVIDER, LOCAL_ROOT, SKILL_ID, imagineHelp, localRoot, logo,
  parseImagineArgs, runImagineCommand, runJustImagine, runServiceCommand,
  skillSource, skillTarget
} = createImagineCli({
  command: 'bro imagine', config, catalogues, ui, state, service,
  charactersRoot: CHARACTERS_DIR, localRoot: ['.bro', 'justimagine'],
  migrateLegacy: true, authRequired: false
});
