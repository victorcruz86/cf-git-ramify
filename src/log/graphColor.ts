import { TOKENS } from "../lib/theme";

const DEFAULT_COLOR = 0;

// `JBColor.BLACK`: in the dark theme it is `Label.foreground`, not black.
const colors = new Map<number, string>([[DEFAULT_COLOR, TOKENS.text]]);

// `%` keeps the sign, as in Kotlin.
const rangeFix = (n: number) => Math.abs(n % 100) + 70;

// `DefaultColorGenerator`: the same color in both themes, except for the default color.
function calculate(id: number): string {
  const red = (Math.imul(id, 200) + 30) | 0;
  const green = (Math.imul(id, 130) + 50) | 0;
  const blue = (Math.imul(id, 90) + 100) | 0;
  return `rgb(${rangeFix(red)},${rangeFix(green)},${rangeFix(blue)})`;
}

export function graphColor(id: number): string {
  let color = colors.get(id);
  if (color === undefined) {
    color = calculate(id);
    colors.set(id, color);
  }
  return color;
}
