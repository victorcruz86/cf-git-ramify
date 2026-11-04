const HEADS = "refs/heads/";

// The target as a branch: in the dialog it can be typed with its full name.
const branchName = (target: string) =>
  target.startsWith(HEADS) ? target.slice(HEADS.length) : target;

function matcher(pattern: string): RegExp | null {
  try {
    new RegExp(pattern);
    return new RegExp(`^(?:${pattern})$`);
  } catch {
    return null;
  }
}

// `GitSharedSettings.isBranchProtected`, to disable buttons and rows: what counts is `push::is_protected`,
// in Rust. A pattern that only one of the two understands shows up here, never in what gets overwritten.
export function isProtectedBranch(patterns: string[], target: string): boolean {
  const branch = branchName(target);
  return patterns.some((pattern) => matcher(pattern)?.test(branch));
}

export const forcePushProhibited = (target: string) =>
  `Force push to ${branchName(target)} is prohibited`;

export const forcePushWarning = (branch: string) =>
  `You're going to force push to ${branch}. It may overwrite commits at the remote. Are you sure you want to proceed?`;

// `force_push_targets`. The colon fits in neither of the two names.
export const silentTarget = (remote: string, branch: string) => `${remote}:${branch}`;
