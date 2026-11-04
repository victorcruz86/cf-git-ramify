// The checkboxes of the Commit view, with the rules of `ChangesViewCommitWorkflowHandler`. `known`
// holds the files of the active list that have already been seen included: what the user unchecks
// stays in `known` and so it is not checked again on its own.
export interface Inclusion {
  included: Set<string>;
  known: Set<string>;
}

function intersect(paths: Set<string>, keep: Set<string>): Set<string> {
  return new Set([...paths].filter((path) => keep.has(path)));
}

// On every refresh: whatever no longer has changes is forgotten. A new file is not checked here.
export function retainInclusion(state: Inclusion, all: string[], active: string[]): Inclusion {
  return {
    included: intersect(state.included, new Set(all)),
    known: intersect(state.known, new Set(active)),
  };
}

export function noteInclusion(state: Inclusion, active: string[]): Inclusion {
  const known = new Set(state.known);
  for (const path of active) {
    if (state.included.has(path)) known.add(path);
  }
  return { included: state.included, known };
}

// `CheckinProject`: includes what is new in the active list, unless files of another list are
// checked.
export function activateInclusion(
  state: Inclusion,
  active: string[],
  unversioned: string[],
): Inclusion {
  const own = new Set([...active, ...unversioned]);
  if ([...state.included].some((path) => !own.has(path))) return state;

  const included = new Set(state.included);
  for (const path of active) {
    if (!state.known.has(path)) included.add(path);
  }
  if (included.size === 0) active.forEach((path) => included.add(path));
  return noteInclusion({ included, known: state.known }, active);
}

// `CheckinFiles`: the chosen files replace whatever was checked.
export function forceInclusion(paths: string[], active: string[]): Inclusion {
  return noteInclusion({ included: new Set(paths), known: new Set() }, active);
}

export function toggleInclusion(included: Set<string>, paths: string[]): Set<string> {
  const next = new Set(included);
  const add = paths.some((path) => !included.has(path));
  for (const path of paths) {
    if (add) next.add(path);
    else next.delete(path);
  }
  return next;
}
