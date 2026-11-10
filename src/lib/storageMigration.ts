// The app was called rama until 2026-11-10 and three of its keys carried that name. The webview's
// storage is copied by the backend (`migration.rs`); here rama's keys are copied to Ramify's once,
// never over a key Ramify already saved, and rama's stay.
const OLD_PREFIX = "rama.";
const NEW_PREFIX = "ramify.";

export function migrateStorageKeys(storage: Storage): void {
  const old: string[] = [];
  for (let index = 0; index < storage.length; index++) {
    const key = storage.key(index);
    if (key?.startsWith(OLD_PREFIX)) old.push(key);
  }
  for (const key of old) {
    const target = NEW_PREFIX + key.slice(OLD_PREFIX.length);
    const value = storage.getItem(key);
    if (value !== null && storage.getItem(target) === null) storage.setItem(target, value);
  }
}
