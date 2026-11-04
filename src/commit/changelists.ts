import type { ChangeList, Changelists } from "../lib/api";

export function nameProblem(name: string, taken: string[]): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Cannot create new changelist with empty name";
  const lower = trimmed.toLowerCase();
  if (taken.some((other) => other.toLowerCase() === lower)) {
    return "A changelist with that name already exists";
  }
  return null;
}

export function findList(lists: Changelists, name: string): ChangeList | undefined {
  const lower = name.trim().toLowerCase();
  return lists.lists.find((list) => list.name.toLowerCase() === lower);
}
