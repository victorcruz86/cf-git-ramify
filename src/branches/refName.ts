// `GitRefNameValidator` and the checks of `GitNewBranchDialog`. It is a typing aid: Rust decides, with
// `git check-ref-format`.
const REPLACE = / +|[\x00-\x1f\x7f]/g;
const DROP = /(^\.)|(^-)|(^\/)|[~:^?*"[\\]+|(@\{)+|\/(?=\/)|(\.(?=\.))+|\.(?=\/)|(?<=\/)\./g;
const ENDS = /([./]|\.lock)$/;
const CLEANUP_SYMBOL = "-";
const HEAD = "HEAD";

export interface BranchNames {
  local: string[];
  remote: string[];
}

export interface NameCheck {
  // Empty is only an error on accept: while the dialog is opening it does not complain.
  onApply: boolean;
  overwrite: boolean;
  showReset: boolean;
}

// While typing, spaces are replaced and what is not valid is removed.
export const cleanOnTyping = (name: string) =>
  name.replace(DROP, "").replace(REPLACE, CLEANUP_SYMBOL);

export const cleanName = (name: string) => cleanOnTyping(name).replace(ENDS, "").trim();

export function isValidRefName(name: string): boolean {
  if (name.trim() === "") return false;
  const dirty = new RegExp(DROP.source).test(name) || ENDS.test(name);
  return !dirty && !new RegExp(REPLACE.source).test(name);
}

function directories(names: string[]): Set<string> {
  const found = new Set<string>();
  for (const name of names) {
    for (let slash = name.indexOf("/"); slash >= 0; slash = name.indexOf("/", slash + 1)) {
      found.add(name.slice(0, slash));
    }
  }
  return found;
}

export const overwriteEnabled = (text: string, names: BranchNames) =>
  names.local.includes(cleanName(text));

// `GitNewBranchDialog.validateBranchName`.
export function branchProblem(
  text: string,
  names: BranchNames,
  { onApply, overwrite, showReset }: NameCheck,
): string | null {
  const name = cleanName(text);
  if (onApply) {
    if (name === "" || text.trim() === "") return "Specify name for the new branch";
    if (name.toUpperCase() === HEAD) return "HEAD is a reserved keyword";
    if (!isValidRefName(name)) return `Branch name ${name} is not valid`;
  }
  if (names.remote.includes(name)) {
    return `Branch name ${name} conflicts with remote branch with the same name`;
  }
  if (directories(names.local).has(name)) {
    return `Branch name ${name} conflicts with local branch directory with the same name`;
  }
  if (!names.local.includes(name) || overwrite) return null;
  const exists = `Branch name ${name} already exists`;
  return showReset ? `${exists}.\nChange the name or overwrite existing branch` : exists;
}
