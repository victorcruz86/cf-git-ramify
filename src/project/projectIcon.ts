// The project's icon: a coloured square with its initials. Ported from IntelliJ IDEA Community
// 2024.1 (tag idea/241.14494.240):
// - `AvatarUtils.initials` (platform/util/ui/src/com/intellij/util/ui/AvatarUtils.kt), which
//   `RecentProjectIconHelper.generateProjectIcon` reaches through `AvatarIcon` with the project's name;
// - `ProjectIconPalette` (platform/platform-impl/src/com/intellij/ide/RecentProjectIconHelper.kt),
//   the nine gradients, in their dark-theme values since Ramify is dark;
// - `ColorPalette.select` (AvatarUtils.kt): `abs(seed.hashCode()) % size`.
// Difference: IntelliJ's `ProjectWindowCustomizerService` gives a project the next index of a
// rotating counter the first time it shows it, and stores it. Ramify keeps no such store, so it
// uses the hash of `ColorPalette.select`, the same project always getting the same colour.

// `RecentProject.ColorN.Avatar.Start` and `.End`, the dark values of `JBColor(light, dark)`.
export const PROJECT_PALETTE: ReadonlyArray<readonly [string, string]> = [
  ["#CE443C", "#E77E41"],
  ["#E27237", "#E8A83E"],
  ["#2DBCAD", "#35D6A4"],
  ["#3895E1", "#51C5EA"],
  ["#7B75E8", "#7D99EB"],
  ["#7854AD", "#897AE6"],
  ["#8F4593", "#B572E3"],
  ["#C840B9", "#E074AE"],
  ["#D75370", "#E96FA3"],
];

const isHighSurrogate = (code: number) => code >= 0xd800 && code <= 0xdbff;
const isLowSurrogate = (code: number) => code >= 0xdc00 && code <= 0xdfff;

const letterOrDigit = /^[\p{L}\p{Nd}]$/u;
const letter = /^\p{L}$/u;
const upper = /^\p{Lu}$/u;

// `generateFromCamelCase`: from the first letter, the letters and digits up to the first other
// character; of those, the first and the capitals; at most two.
function fromCamelCase(text: string): string {
  const chars = Array.from(text);
  const start = chars.findIndex((char) => letter.test(char));
  if (start < 0) return "";
  const run: string[] = [];
  for (const char of chars.slice(start)) {
    if (!letterOrDigit.test(char)) break;
    run.push(char);
  }
  return run
    .filter((char, index) => index === 0 || upper.test(char))
    .slice(0, 2)
    .join("")
    .toUpperCase();
}

// `splitAtLeast2NonEmpty`: the pieces cut at the delimiters, without their characters that are
// neither letters nor digits, and without the empty ones; `null` if fewer than two are left.
function splitAtLeast2(text: string, delimiters: string): string[] | null {
  const pieces: string[] = [];
  let current = "";
  const flush = () => {
    const piece = Array.from(current)
      .filter((char) => letterOrDigit.test(char))
      .join("");
    if (piece) pieces.push(piece);
    current = "";
  };
  for (const char of text) {
    if (delimiters.includes(char)) flush();
    else current += char;
  }
  flush();
  return pieces.length >= 2 ? pieces : null;
}

// Examples: "John Smith" gives "JS", "John-Smith-Harris" "JH", "MyProject" "MP", "ramify" "R".
export function projectInitials(name: string): string {
  let filtered = "";
  for (let index = 0; index < name.length; index++) {
    const code = name.charCodeAt(index);
    if (!isHighSurrogate(code) && !isLowSurrogate(code)) filtered += name[index];
  }
  filtered = filtered.trim();
  const camelCase = fromCamelCase(name);
  if (camelCase.length === 2) return camelCase;
  const words =
    splitAtLeast2(filtered, " ") ??
    splitAtLeast2(filtered, ",") ??
    splitAtLeast2(filtered, "-") ??
    splitAtLeast2(filtered, "_") ??
    splitAtLeast2(filtered, ".") ??
    splitAtLeast2(filtered, "`'\"");
  if (!words) return camelCase;
  return (words[0][0] + words[words.length - 1][0]).toUpperCase();
}

// Java's `String.hashCode`: 32-bit, over the UTF-16 units.
function javaHash(text: string): number {
  let hash = 0;
  for (let index = 0; index < text.length; index++) {
    hash = (Math.imul(hash, 31) + text.charCodeAt(index)) | 0;
  }
  return hash;
}

// `ColorPalette.select` with the project's path as the seed. Kotlin's `abs(Int.MIN_VALUE)` stays
// negative and would give a negative index there; the index here is kept inside the palette.
export function projectColorIndex(path: string): number {
  return Math.abs(Math.abs(javaHash(path)) % PROJECT_PALETTE.length);
}
