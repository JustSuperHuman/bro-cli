// Treat anything "commented out" with a leading '#' as ignored test data / notes.
//
//   - object keys starting with '#'                 -> removed
//   - array string items starting with '#'          -> removed
//   - array object items whose id/name starts '#'   -> removed
//   - array object items left empty after cleaning  -> removed (e.g. { "#id": "x" })
//
// Applied to both the remote models.json and the user's ~/.bro/config.json.
export function stripHash(value) {
  if (Array.isArray(value)) {
    return value.map(stripHash).filter((item) => {
      if (item == null) return false;
      if (typeof item === 'string') return !item.trimStart().startsWith('#');
      if (typeof item === 'object' && !Array.isArray(item)) {
        if (Object.keys(item).length === 0) return false;
        const id = item.id ?? item.name;
        if (typeof id === 'string' && id.trimStart().startsWith('#')) return false;
      }
      return true;
    });
  }
  if (value && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) {
      if (k.startsWith('#')) continue;
      out[k] = stripHash(v);
    }
    return out;
  }
  return value;
}
