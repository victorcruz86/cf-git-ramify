import { cleanup, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CommitDetails, Signature, SignatureStatus, TaskEvent } from "../lib/api";
import { trackTasks } from "../lib/tasks";
import DetailsPanel from "./DetailsPanel.svelte";
import { signatureDescription, signatureIcon, signatureText } from "./signatures";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

const hashOf = (index: number) => index.toString(16).padStart(40, "0");

const details = (hash: string): CommitDetails => ({
  hash,
  subject: `subject ${hash.slice(-2)}`,
  body: "",
  author: "Ana",
  authorEmail: "ana@example.com",
  authorTime: 100,
  committer: "Ana",
  committerEmail: "ana@example.com",
  commitTime: 100,
  samePerson: true,
  refs: [],
  parents: [],
  links: {},
});

const signature = (index: number, status: SignatureStatus): Signature => ({
  hash: hashOf(index),
  status,
  signer: status === "verified" ? "someone@example.com" : "",
  fingerprint: "SHA256:abc",
});

let onTask: ((event: { payload: TaskEvent }) => void) | null;
let asked: { id: number; hashes: string[] }[];
let answers: Signature[];

beforeEach(async () => {
  onTask = null;
  asked = [];
  answers = [];
  vi.mocked(listen).mockImplementation(async (_, handler) => {
    onTask = handler as never;
    return () => {};
  });
  await trackTasks();
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const values = args as Record<string, unknown>;
    if (command === "commit_details") return (values.hashes as string[]).map(details);
    if (command === "branches_containing") return [];
    if (command === "task_cancel") return undefined;
    if (command === "log_signatures") {
      const id = asked.length + 1;
      asked.push({ id, hashes: values.hashes as string[] });
      // The task's answer comes after the command returns.
      setTimeout(() =>
        onTask?.({
          payload: {
            id,
            title: "",
            text: "",
            fraction: null,
            state: "done",
            kind: null,
            value: answers,
          },
        }),
      );
      return id;
    }
    throw new Error(`unexpected command: ${command}`);
  });
});

afterEach(cleanup);

const articleOf = (index: number) =>
  screen.getByText(`subject ${hashOf(index).slice(-2)}`).closest("article") as HTMLElement;

// P§2.3 and P§2.4: the cell's tooltip and the details' text are the presentation's `text`; the
// details' icon has the description as its tooltip, with the fingerprint and the signer only for a
// verified one; `N` has no line at all.
test("tooltips_and_details_follow_241s_texts_per_state", async () => {
  expect(signatureText(undefined)).toBe("No GPG signature");
  expect(signatureIcon(undefined)).toBeNull();
  const texts = (["verified", "unknown", "expired", "expiredKey", "revokedKey", "cannotVerify", "bad"] as const).map(
    (status, index) => {
      const shown = signature(index, status);
      return [signatureIcon(shown), signatureText(shown), signatureDescription(shown).text];
    },
  );
  expect(texts).toEqual([
    ["signatureVerified", "Verified GPG signature", "Verified GPG signature"],
    ["signatureSigned", "Unverified GPG signature", "Unverified GPG signature: unknown validity"],
    ["signatureSigned", "Unverified GPG signature", "Unverified GPG signature: expired signature"],
    ["signatureSigned", "Unverified GPG signature", "Unverified GPG signature: made with expired key"],
    ["signatureSigned", "Unverified GPG signature", "Unverified GPG signature: made with revoked key"],
    ["signatureSigned", "Unverified GPG signature", "Unverified GPG signature: cannot verify (e.g. missing key)"],
    ["signatureSigned", "Bad GPG signature", "Bad GPG signature"],
  ]);

  answers = [signature(1, "verified"), signature(2, "unknown"), signature(3, "bad")];
  render(DetailsPanel, {
    hashes: [hashOf(1), hashOf(2), hashOf(3), hashOf(4)],
    generation: 0,
    onnavigate: vi.fn(),
  });
  // The text next to the icon; the tooltip is the description.
  const textOf = (index: number) =>
    articleOf(index).querySelector(".signature > span:last-child")?.textContent ?? null;
  await waitFor(() => expect(textOf(1)).toBe("Verified GPG signature"));

  expect(asked.map(({ hashes }) => hashes)).toEqual([[hashOf(1), hashOf(2), hashOf(3), hashOf(4)]]);
  const tooltip = (index: number) =>
    [...within(articleOf(index)).getByRole("tooltip", { hidden: true }).querySelectorAll(".line")].map(
      (line) => line.textContent,
    );
  expect(tooltip(1)).toEqual([
    "Verified GPG signature",
    "Signature fingerprint:",
    "SHA256:abc",
    "Signed by:",
    "someone@example.com",
  ]);
  const context = within(articleOf(1)).getByText("Signed by:").closest(".context");
  expect(context?.textContent).toContain("SHA256:abc");
  expect(textOf(2)).toBe("Unverified GPG signature");
  expect(tooltip(2)).toEqual(["Unverified GPG signature: unknown validity"]);
  expect(textOf(3)).toBe("Bad GPG signature");
  expect(tooltip(3)).toEqual(["Bad GPG signature"]);
  // Unsigned: no signature line, not even `No GPG signature`.
  expect(within(articleOf(4)).queryByRole("tooltip", { hidden: true })).toBeNull();
  expect(articleOf(4).querySelector(".signature")).toBeNull();
  expect(articleOf(4).textContent).not.toContain("GPG signature");
  // After the committer line, which here is the author's.
  const lines = [...articleOf(1).querySelectorAll("p")].map((line) => line.className);
  expect(lines.indexOf(lines.find((name) => name.includes("signature"))!)).toBe(2);
});

// Review Focus 3: the details ask for the commits they show, at most `MAX_COMMITS`; the dialogs
// that use the panel without `In branches:` ask for nothing.
test("the_details_ask_for_at_most_the_shown_commits_and_the_dialogs_for_none", async () => {
  const many = Array.from({ length: 60 }, (_, index) => hashOf(index));
  const shown = render(DetailsPanel, { hashes: many, generation: 0, onnavigate: vi.fn() });
  await waitFor(() => expect(asked).toHaveLength(1));
  expect(asked[0].hashes).toEqual(many.slice(0, 50));
  shown.unmount();

  render(DetailsPanel, { hashes: [hashOf(1)], generation: 0, onnavigate: vi.fn(), showBranches: false });
  await screen.findByText("subject 01");
  await new Promise((resolve) => setTimeout(resolve, 10));
  expect(asked).toHaveLength(1);
});
