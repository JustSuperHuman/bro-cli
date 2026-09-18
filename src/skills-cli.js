import fs from 'node:fs';
import path from 'node:path';
import { consolidate } from './skills-consolidator.js';

export function consolidateSkills(root, names = [], options = {}) {
  return consolidate(root, names, options);
}
