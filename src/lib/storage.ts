// What a view remembers between sessions. Without storage it is simply not remembered.
export function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Without storage, all that is lost is what is remembered between sessions.
  }
}
