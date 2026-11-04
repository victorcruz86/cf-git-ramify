import type { UpdateMethod } from "../lib/api";

// `GitUpdateOptionsPanel`.
export const UPDATE_METHODS: { id: UpdateMethod; label: string }[] = [
  { id: "merge", label: "Merge incoming changes into the current branch" },
  { id: "rebase", label: "Rebase the current branch on top of incoming changes" },
];
