import type { AccountView, Connection, MappingView } from "../lib/api";

// What the Pull Requests tests build their backend answers from.
export const mapping = (display: string, remote: string, server = "github.com"): MappingView => ({
  remote,
  url: `https://${server}/${display}.git`,
  display,
  server,
  repository: `${server}/${display}`,
});

export const account = (login: string, extra: Partial<AccountView> = {}): AccountView => ({
  id: `${extra.server ?? "github.com"}/${login}`,
  server: "github.com",
  login,
  name: null,
  error: null,
  needsLogin: false,
  isDefault: false,
  ...extra,
});

export const connection = (remote: string, accountId: string): Connection => ({
  remote,
  url: `https://github.com/o/r.git`,
  account: accountId,
  coordinates: {
    server: { useHttp: null, host: "github.com", port: null, suffix: null },
    owner: "o",
    name: "r",
  },
  viewerLogin: accountId.split("/").pop()!,
  repository: {
    id: "R_1",
    owner: "o",
    nameWithOwner: "o/r",
    viewerPermission: "WRITE",
    mergeCommitAllowed: true,
    squashMergeAllowed: true,
    rebaseMergeAllowed: true,
    defaultBranch: "main",
    isFork: false,
    url: "https://github.com/o/r",
    sshUrl: "git@github.com:o/r.git",
  },
  title: "r",
});

