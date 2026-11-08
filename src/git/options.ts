// The options of the Merge… and Rebase… dialogs: their texts, which ones cannot go together and which version
// of git knows them. Ported from `GitMergeOption`, `GitRebaseOption`, `GitRebaseDialog.isOptionEnabled`
// and `GitVersionSpecialty`.
import type { MergeOption } from "../lib/api";

export const MERGE_OPTIONS: { id: MergeOption; flag: string; description: string }[] = [
  {
    id: "noFf",
    flag: "--no-ff",
    description: "Create a merge commit even if it can be fast-forwarded",
  },
  { id: "ffOnly", flag: "--ff-only", description: "Merge only if it can be fast-forwarded" },
  {
    id: "squash",
    flag: "--squash",
    description: "Create a single commit for all merged changes",
  },
  { id: "message", flag: "-m", description: "Specify a message for the merge commit" },
  { id: "noCommit", flag: "--no-commit", description: "Do not commit the merge result" },
  {
    id: "noVerify",
    flag: "--no-verify",
    description: "Bypass the pre-merge and commit message hooks",
  },
  {
    id: "allowUnrelatedHistories",
    flag: "--allow-unrelated-histories",
    description: "Allow merging histories that do not share a common ancestor.",
  },
];

// `GitMergeOption.isOptionSuitable`: with each one chosen, the ones that cannot go with it.
const MERGE_CLASHES: Record<MergeOption, MergeOption[]> = {
  noFf: ["ffOnly", "squash"],
  ffOnly: ["noFf", "squash", "message"],
  squash: ["noFf", "ffOnly", "message"],
  message: ["ffOnly", "squash", "noCommit"],
  noCommit: ["message"],
  noVerify: [],
  allowUnrelatedHistories: [],
};

// The ones that can be chosen given what is already chosen (`isOptionEnabled`). What is chosen never clashes
// with itself, so it can always be removed.
export function mergeOptionsCompatible(selected: Iterable<MergeOption>): Set<MergeOption> {
  const clashing = new Set([...selected].flatMap((option) => MERGE_CLASHES[option]));
  return new Set(MERGE_OPTIONS.map(({ id }) => id).filter((id) => !clashing.has(id)));
}

// `[major, minor, patch]`.
export type GitVersion = [number, number, number];

export function parseGitVersion(text: string): GitVersion | null {
  const found = /(\d+)\.(\d+)(?:\.(\d+))?/.exec(text);
  return found ? [Number(found[1]), Number(found[2]), Number(found[3] ?? 0)] : null;
}

// Without knowing the version it counts as the oldest (`GitVersion.NULL`).
function atLeast(version: GitVersion | null, major: number, minor: number): boolean {
  return version !== null && (version[0] > major || (version[0] === major && version[1] >= minor));
}

// `NO_VERIFY_SUPPORTED`.
export const noVerifySupported = (version: GitVersion | null) => atLeast(version, 2, 24);

// `REBASE_MERGES_REPLACES_PRESERVE_MERGES`.
export const rebaseMergesAvailable = (version: GitVersion | null) => atLeast(version, 2, 22);

export function mergeOptionsAvailable(version: GitVersion | null): typeof MERGE_OPTIONS {
  return MERGE_OPTIONS.filter(({ id }) => id !== "noVerify" || noVerifySupported(version));
}

// `GitMergeDialog.loadSettings`: the remembered options that this version of git knows, in the order
// of the enum as an `EnumSet` returns them. What is incompatible stays.
export function rememberedMergeOptions(
  saved: MergeOption[],
  version: GitVersion | null,
): MergeOption[] {
  const known = new Set(mergeOptionsAvailable(version).map(({ id }) => id));
  return MERGE_OPTIONS.map(({ id }) => id).filter((id) => known.has(id) && saved.includes(id));
}

// `GitRebaseOption`. `switchBranch` and `onto` are not flags of the command: they open their field.
export type RebaseOption =
  | "switchBranch"
  | "onto"
  | "rebaseMerges"
  | "keepEmpty"
  | "root"
  | "interactive"
  | "updateRefs";

export const REBASE_OPTIONS: {
  id: RebaseOption;
  description: string;
  // `hasSeparatorAbove`.
  separatorAbove?: boolean;
}[] = [
  { id: "switchBranch", description: "Select another branch to rebase" },
  { id: "onto", description: "Specify a new base for the rebased commits" },
  { id: "rebaseMerges", description: "Recreate commits topology" },
  { id: "keepEmpty", description: "Do not remove empty commits during rebase" },
  { id: "root", description: "Rebase all commits in the branch" },
  { id: "interactive", description: "Edit commits before rebasing", separatorAbove: true },
  { id: "updateRefs", description: "Auto-update branches pointing to rebased commits" },
];

// `GitRebaseOption.getOption`: `--rebase-merges` replaced `--preserve-merges` in git 2.22.
export function rebaseFlagText(option: RebaseOption, version: GitVersion | null): string {
  switch (option) {
    case "switchBranch":
      return "";
    case "onto":
      return "--onto";
    case "rebaseMerges":
      return rebaseMergesAvailable(version) ? "--rebase-merges" : "--preserve-merges";
    case "keepEmpty":
      return "--keep-empty";
    case "root":
      return "--root";
    case "interactive":
      return "--interactive";
    case "updateRefs":
      return "--update-refs";
  }
}

// `GitRebaseDialog.isOptionEnabled`: without `--rebase-merges`, `--preserve-merges` does not go with
// `--interactive`.
export function rebaseOptionsCompatible(
  selected: Iterable<RebaseOption>,
  version: GitVersion | null,
): Set<RebaseOption> {
  const chosen = new Set(selected);
  const all = REBASE_OPTIONS.map(({ id }) => id);
  if (rebaseMergesAvailable(version)) return new Set(all);
  return new Set(
    all.filter(
      (id) =>
        !(id === "rebaseMerges" && chosen.has("interactive")) &&
        !(id === "interactive" && chosen.has("rebaseMerges")),
    ),
  );
}
