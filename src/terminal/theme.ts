import type { ITerminalOptions } from "@xterm/xterm";
import { TOKENS } from "../lib/theme";

// The editor's background and text, and the console colours of the dark scheme (Darcula's
// CONSOLE_*_OUTPUT), close to what IntelliJ's terminal shows. Not compared value by value with 241.
export const TERMINAL_OPTIONS: ITerminalOptions = {
  fontFamily: '"JetBrains Mono", Menlo, Monaco, monospace',
  fontSize: 13,
  lineHeight: 1.2,
  cursorBlink: true,
  scrollback: 10_000,
  macOptionIsMeta: false,
  theme: {
    background: TOKENS.editor,
    foreground: TOKENS["editor-text"],
    cursor: TOKENS.caret,
    selectionBackground: "#214283",
    black: "#000000",
    red: "#F0524F",
    green: "#5C962C",
    yellow: "#A68A0D",
    blue: "#3993D4",
    magenta: "#A771BF",
    cyan: "#00A3A3",
    white: "#808080",
    brightBlack: "#595959",
    brightRed: "#FF4050",
    brightGreen: "#4FC414",
    brightYellow: "#E5BF00",
    brightBlue: "#1FB0FF",
    brightMagenta: "#ED7EED",
    brightCyan: "#00E5E5",
    brightWhite: "#FFFFFF",
  },
};
