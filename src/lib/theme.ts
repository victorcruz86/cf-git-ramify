// New UI dark theme in version 232.10072.27.
// `ui` and palette: platform-resources/src/themes/expUI/expUI_dark.theme.json.
// Editor: themes/expUI/expUI_darkScheme.xml, which inherits the FILESTATUS_* of Darcula in
// DefaultColorSchemesManager.xml.
const GREY = {
  1: "#1E1F22",
  2: "#2B2D30",
  3: "#393B40",
  4: "#43454A",
  5: "#4E5157",
  6: "#5A5D63",
  7: "#6F737A",
  9: "#9DA0A8",
  11: "#CED0D6",
  12: "#DFE1E5",
  13: "#F0F1F2",
  14: "#FFFFFF",
};

const BLUE = { 2: "#2E436E", 6: "#3574F0", 8: "#548AF7", 9: "#6B9BFA" };
const GREEN = { 1: "#253627", 2: "#375239", 5: "#57965C" };
const YELLOW = { 1: "#3D3223", 2: "#5E4D33", 7: "#F2C55C" };
const RED = { 2: "#5E3838", 3: "#7A4343", 6: "#DB5C5C" };

export const TOKENS = {
  // Surfaces. `panel` is *.background; `editor`, the TEXT background.
  panel: GREY[2],
  editor: GREY[1],
  // Borders.color separates windows; OnePixelDivider.background, areas of the same window.
  border: GREY[1],
  divider: GREY[3],
  // Texts. `muted` is infoForeground, the gray of counters and paths.
  text: GREY[12],
  "text-strong": GREY[14],
  muted: GREY[7],
  "text-secondary": GREY[9],
  disabled: GREY[6],
  "editor-text": "#BCBEC4",
  caret: GREY[11],
  "right-margin": GREY[3],
  icon: GREY[11],
  shortcut: GREY[7],
  link: BLUE[9],
  // Interaction.
  hover: GREY[3],
  pressed: GREY[5],
  selection: BLUE[2],
  "selection-inactive": GREY[4],
  focus: BLUE[6],
  accent: BLUE[6],
  "accent-text": GREY[14],
  // Controls.
  "control-border": GREY[5],
  "control-disabled-border": GREY[4],
  "input-background": GREY[2],
  // checkBoxTemp.svg: the real border of the checkbox, lighter than Checkbox.Border.Default.Dark.
  "checkbox-border": GREY[7],
  "popup-background": GREY[2],
  "popup-border": GREY[4],
  "menu-separator": GREY[4],
  "stripe-selected": BLUE[6],
  backdrop: "#00000066",
  "scroll-thumb": "#FFFFFF26",
  "scroll-thumb-hover": "#FFFFFF4D",
  // File status.
  "status-modified": "#6897BB",
  "status-added": "#629755",
  "status-deleted": "#6C6C6C",
  "status-unversioned": "#D1675A",
  "status-conflict": "#D5756C",
  "status-ignored": "#848504",
  // FILESTATUS_* of the 241 dark New UI scheme (`expUI_darkScheme.xml:55-74`): the Project tree.
  "tree-status-added": "#73BD79",
  "tree-status-modified": "#70AEFF",
  "tree-status-deleted": "#6F737A",
  "tree-status-unknown": "#E88F89",
  "tree-status-ignored": "#D69A6B",
  "tree-status-conflict": "#DE6A66",
  // Messages. Notification.ToolWindow.*
  "notice-info": GREEN[1],
  "notice-info-border": GREEN[2],
  "notice-warning": YELLOW[1],
  "notice-warning-border": YELLOW[2],
  "notice-error": RED[2],
  "notice-error-border": RED[3],
  "notification-background": GREY[3],
  "notification-border": GREY[4],
  "notification-text": GREY[13],
  error: RED[6],
  // RunWidget.* : the merge and rebase widget. `runningBackground` is Green6 of expUI_dark, the icon is
  // Grey12 (`iconColor`), `stopBackground` is the one of darcula.theme.json (expUI has none), and
  // the hover and pressed overlays are `hoverBackground` and `pressedBackground`.
  "run-running": "#57965C",
  "run-stop": "#C94F4F",
  "run-icon": GREY[12],
  "run-hover": "#00000019",
  "run-pressed": "#00000028",
  // WARNING_ATTRIBUTES and ERRORS_ATTRIBUTES of the editor.
  "editor-warning": YELLOW[7],
  // Diff editor: colors from expUI_darkScheme.xml.
  "editor-line-number": "#4B5059",
  "editor-line-number-current": "#A1A3AB",
  "editor-current-line": "#26282E",
  "editor-folded-text": "#868991",
  "editor-folded": GREY[3],
  "diff-separator": "#2B2D30",
  // DIFF_* of Darcula in DefaultColorSchemesManager.xml, which the dark scheme inherits. `-line` is
  // the background of the lines with highlighted words: TextDiffType.getIgnoredColor, the color
  // blended at 60 % with the editor background.
  "diff-inserted": "#294436",
  "diff-deleted": "#484A4A",
  "diff-modified": "#385570",
  "diff-modified-line": "#283541",
  "diff-inserted-stripe": "#447152",
  "diff-deleted-stripe": "#656E76",
  "diff-modified-stripe": "#43698D",
  // DIFF_CONFLICT, and the word line backgrounds of the other types, for the merge.
  "diff-conflict": "#45302B",
  "diff-conflict-stripe": "#8F5247",
  "diff-conflict-line": "#2E2626",
  "diff-inserted-line": "#222E2A",
  "diff-deleted-line": "#2F3032",
  // VersionControl.Log.Commit.currentBranchBackground and unmatchedForeground.
  "log-current-branch": "#1c2433",
  "log-merge": GREY[7],
  // The `GPG Signature` icons, Ramify's own drawings (decision 6): a green badge for a verified
  // signature, an amber one for the other signed states.
  "signature-verified": GREEN[5],
  "signature-signed": YELLOW[7],
};

export function applyTheme(root: HTMLElement): void {
  for (const [name, value] of Object.entries(TOKENS)) {
    root.style.setProperty(`--${name}`, value);
  }
}
