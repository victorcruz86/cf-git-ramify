import type { PullInfo, PullOption } from "../lib/api";

// `GitPullOption`, in their order.
export const PULL_OPTIONS: { id: PullOption; flag: string; description: string }[] = [
  {
    id: "rebase",
    flag: "--rebase",
    description: "Rebase incoming changes on top of the current branch",
  },
  { id: "ffOnly", flag: "--ff-only", description: "Merge only if it can be fast-forwarded" },
  {
    id: "noFf",
    flag: "--no-ff",
    description: "Create a merge commit even if it can be fast-forwarded",
  },
  {
    id: "squash",
    flag: "--squash",
    description: "Create a single commit for all pulled changes",
  },
  { id: "noCommit", flag: "--no-commit", description: "Merge, but do not commit the result" },
  {
    id: "noVerify",
    flag: "--no-verify",
    description: "Bypass the pre-merge and commit message hooks",
  },
];

// `GitPullOption.isOptionSuitable`: with each one chosen, the ones that cannot go with it.
const CLASHES: Record<PullOption, PullOption[]> = {
  rebase: ["ffOnly", "noFf", "squash", "noCommit"],
  ffOnly: ["noFf", "squash", "rebase"],
  noFf: ["ffOnly", "squash", "rebase"],
  squash: ["noFf", "ffOnly", "rebase"],
  noCommit: ["rebase"],
  noVerify: [],
};

export function incompatible(chosen: Iterable<PullOption>): Set<PullOption> {
  return new Set([...chosen].flatMap((option) => CLASHES[option]));
}

export const pullTitle = (branch: string | null) => (branch ? `Pull to ${branch}` : "Pull");

// The longest remote that is a prefix: `a` and `a/b` can coexist.
function remoteOf(full: string, remotes: string[]): string | null {
  const owners = remotes.filter((remote) => full.startsWith(`${remote}/`));
  return owners.reduce<string | null>(
    (best, remote) => (best === null || remote.length > best.length ? remote : best),
    null,
  );
}

export function branchesOf(
  info: Pick<PullInfo, "remotes" | "remoteBranches">,
  remote: string | null,
): string[] {
  if (remote === null) return [];
  return info.remoteBranches.flatMap((full) =>
    remoteOf(full, info.remotes) === remote ? [full.slice(remote.length + 1)] : [],
  );
}

// `getCurrentOrDefaultRemote`: the tracked one, or `origin`, or the first.
export function chooseRemote({ remotes, tracked }: PullInfo): string | null {
  if (remotes.length === 0) return null;
  if (tracked && remotes.includes(tracked[0])) return tracked[0];
  return remotes.includes("origin") ? "origin" : remotes[0];
}

// `updateBranchesField`: what was already chosen if the remote has it, otherwise the tracked branch
// (even if it is from another remote, like IntelliJ), otherwise the one with the same name.
export function chooseBranch(info: PullInfo, remote: string | null, previous: string | null) {
  const branches = branchesOf(info, remote);
  if (previous && branches.includes(previous)) return previous;
  if (info.tracked) return info.tracked[1];
  return branches.find((name) => name === info.branch) ?? "";
}

export interface SelectionProblem {
  field: "remote" | "branch";
  text: string;
}

// `validateRemoteField`, `validateBranchField` y `validateBranchExists`.
export function selectionProblem(
  info: PullInfo,
  remote: string | null,
  branch: string,
): SelectionProblem | null {
  if (remote === null) return { field: "remote", text: "Select the remote to pull from" };
  if (!branch) return { field: "branch", text: "The branch to pull from should be selected." };
  if (!branchesOf(info, remote).includes(branch)) {
    return { field: "branch", text: "There is no matching branch." };
  }
  return null;
}
