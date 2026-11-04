const WORD_START = (text: string, index: number) =>
  index === 0 ||
  !/[\p{L}\p{N}]/u.test(text[index - 1]) ||
  (/\p{Lu}/u.test(text[index]) && /\p{Ll}/u.test(text[index - 1]));

// `PreferStartMatchMatcherWrapper.MATCH_OFFSET`.
const MATCH_OFFSET = 10000;
// What `MinusculeMatcherImpl.matchingDegree` gives for starting at a word start.
const WORD_START_DEGREE = 1000;
const FRAGMENT_PENALTY = 10;

export interface SpeedMatch {
  // Pieces of the text that match, as [start, end).
  fragments: [number, number][];
  degree: number;
}

function joined(positions: number[]): [number, number][] {
  const fragments: [number, number][] = [];
  for (const position of positions) {
    const last = fragments[fragments.length - 1];
    if (last?.[1] === position) last[1] = position + 1;
    else fragments.push([position, position + 1]);
  }
  return fragments;
}

function wholeText(text: string, lower: string, wanted: string): number {
  let found = lower.indexOf(wanted);
  const first = found;
  while (found >= 0 && !WORD_START(text, found)) found = lower.indexOf(wanted, found + 1);
  return found >= 0 ? found : first;
}

// The degree is not that of `MinusculeMatcherImpl`, but it orders what matters here the same way:
// starting at a word, starting earlier and breaking into fewer pieces.
function degreeOf(text: string, fragments: [number, number][]): number {
  const start = fragments[0][0];
  const word = WORD_START(text, start) ? WORD_START_DEGREE : 0;
  return MATCH_OFFSET - start + word - fragments.length * FRAGMENT_PENALTY;
}

// What the `NameUtil.buildMatcher("*" + text)` of speed search does: case-insensitive, the
// contiguous text or loose letters at word starts.
export function speedMatch(text: string, query: string): SpeedMatch | null {
  const lower = text.toLowerCase();
  const wanted = query.toLowerCase();
  const start = wanted ? wholeText(text, lower, wanted) : -1;
  if (start >= 0) {
    const fragments: [number, number][] = [[start, start + wanted.length]];
    return { fragments, degree: degreeOf(text, fragments) };
  }
  const positions = letters(text, lower, wanted);
  if (!positions || positions.length === 0) return null;
  const fragments = joined(positions);
  return { fragments, degree: degreeOf(text, fragments) };
}

export const speedMatches = (text: string, query: string) =>
  query === "" || speedMatch(text, query) !== null;

function letters(text: string, lower: string, wanted: string): number[] | null {
  let position = 0;
  const positions: number[] = [];
  const wordWith = (letter: string) => {
    let found = lower.indexOf(letter, position);
    while (found >= 0 && !WORD_START(text, found)) found = lower.indexOf(letter, found + 1);
    return found;
  };
  for (const letter of wanted) {
    // A letter follows the previous one or opens a word.
    const next = lower[position] === letter && position > 0 ? position : wordWith(letter);
    if (next < 0) return null;
    positions.push(next);
    position = next + 1;
  }
  return positions;
}
