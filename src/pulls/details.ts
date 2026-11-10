import type { CiJob, PullRequestCommit, PullRequestDetails, PullRequestStatus, ReviewerState } from "../lib/api";

// What a pull request's tab shows, as text (P§6.2-6.9): GHPRDetailsViewModel,
// CodeReviewDetailsStatusComponentFactory, GHPRStatusChecksComponentFactory,
// CodeReviewDetailsCommitsComponentFactory and GHPRDiffVirtualFile (plugins/github and
// platform/collaboration-tools at idea/241.14494.240), with CollaborationToolsBundle (`CTB`) and
// GithubBundle (`GB`) texts.

// `CTB:84-86`: a draft says so over its state, and an open pull request shows none.
export function stateTag(details: Pick<PullRequestDetails, "state" | "isDraft">): string | null {
  if (details.isDraft) return "DRAFT";
  if (details.state === "CLOSED") return "CLOSED";
  if (details.state === "MERGED") return "MERGED";
  return null;
}

// `GB:246`, `{0,number,#}`: no digit grouping.
export const diffTitle = (number: number) => `Diff for Pull Request #${Math.trunc(number)}`;

// The icon of a status line.
export type StatusIcon = "success" | "failed" | "failedInProgress" | "pending" | "skipped" | "warning";

// `calcPipelineIcon` and `calcPipelineText`, first match: every required job passed (also when
// none is required), some pending and some failed, some pending, else failed. `null` with no jobs.
export function ciSummary(jobs: CiJob[]): { icon: StatusIcon; text: string } | null {
  if (jobs.length === 0) return null;
  const pending = jobs.filter((job) => job.state === "PENDING").length;
  const failed = jobs.filter((job) => job.state === "FAILED").length;
  if (jobs.filter((job) => job.isRequired).every((job) => job.state === "SUCCESS")) {
    return { icon: "success", text: "All required checks have passed" };
  }
  if (pending > 0 && failed > 0) {
    return { icon: "failedInProgress", text: "Checks in progress, and some already failed…" };
  }
  if (pending > 0) return { icon: "pending", text: "Checks in progress…" };
  return { icon: "failed", text: "Checks failed" };
}

// `CodeReviewCIJobState.convertToIcon`.
const JOB_ICONS: Record<CiJob["state"], StatusIcon> = {
  FAILED: "failed",
  PENDING: "pending",
  SKIPPED: "skipped",
  SUCCESS: "success",
};
export const jobIcon = (job: CiJob): StatusIcon => JOB_ICONS[job.state];

// `ReviewDetailsUIUtil.getReviewStateIcon` and `getReviewStateText` (`CTB:97-101`).
const REVIEW_ICONS: Record<ReviewerState, StatusIcon> = {
  ACCEPTED: "success",
  WAIT_FOR_UPDATES: "failed",
  NEED_REVIEW: "warning",
};

function reviewText(state: ReviewerState, name: string): string {
  if (state === "ACCEPTED") return `${name} approved review`;
  if (state === "WAIT_FOR_UPDATES") return `${name} is waiting for updates`;
  return `Waiting for review from ${name}`;
}

// A line of the status: `ci` has the `Details…` link.
export interface StatusLine {
  kind: "ci" | "conflicts" | "approvals" | "restricted" | "missing" | "reviewer";
  icon: StatusIcon;
  text: string;
}

// The lines of GHPRStatusChecksComponentFactory in order, without the access lines (decision 7).
// `null` while the status loads: then 241 shows only `Loading pull request status` (`GB:234`).
export function statusLines(details: PullRequestDetails, status: PullRequestStatus | null): StatusLine[] | null {
  if (status === null) return null;
  const lines: StatusLine[] = [];
  const ci = ciSummary(status.jobs);
  if (ci) lines.push({ kind: "ci", ...ci });
  if (status.hasConflicts === true) {
    lines.push({ kind: "conflicts", icon: "failed", text: "Branch has conflicts that must be resolved" });
  }
  if (status.requiredApprovals > 0 && !details.isDraft) {
    lines.push({
      kind: "approvals",
      icon: "failed",
      text: `At least ${status.requiredApprovals} approving review is required by reviewers with write access`,
    });
  }
  if (status.isRestricted && !details.isDraft) {
    lines.push({ kind: "restricted", icon: "failed", text: "You are not authorized to merge request into this branch" });
  }
  if (details.reviewers.length === 0) {
    lines.push({ kind: "missing", icon: "warning", text: "Missing reviewer" });
  }
  for (const reviewer of details.reviewers) {
    lines.push({ kind: "reviewer", icon: REVIEW_ICONS[reviewer.state], text: reviewText(reviewer.state, reviewer.name) });
  }
  return lines;
}

export const LOADING_STATUS = "Loading pull request status";

// The commits dropdown (`CTB:78-81`): the bold title, `{0, choice, 1#Changes|2#Changes from}`, and
// the link, the chosen commit's short hash or the count.
export const commitsTitle = (count: number) => (count >= 2 ? "Changes from" : "Changes");

export function commitsLink(commits: PullRequestCommit[], selection: number | null): string {
  const chosen = selection === null ? undefined : commits[selection];
  return chosen ? chosen.abbreviatedOid : `${commits.length} commits`;
}

export const allCommitsText = (count: number) => `All commits (${count})`;
