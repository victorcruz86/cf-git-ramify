import { get } from "svelte/store";
import { expect, test } from "vitest";
import type { Connection, GithubServerPath } from "../lib/api";
import { onWindowActivationRequested } from "../shell/toolWindows";
import { connection } from "./fixtures";
import {
  connected,
  LIST_TAB,
  openableUrl,
  openPullRequestTab,
  pullsStore,
  refusedUrlText,
  viewPullRequestList,
} from "./store";

const on = (server: Partial<GithubServerPath>): Connection => {
  const base = connection("origin", "github.com/octocat");
  return {
    ...base,
    coordinates: {
      ...base.coordinates,
      server: { useHttp: null, host: "github.com", port: null, suffix: null, ...server },
    },
  };
};

// The Task 2 ruling: a URL from GitHub opens only as https (or http on an http:// server), on
// the connection's host and port, with no credentials.
test("a_url_from_github_opens_only_on_the_connections_server", () => {
  const dotcom = on({});
  const http = on({ useHttp: true, host: "ghe.example.com", port: 8080 });
  const https = on({ useHttp: false, host: "ghe.example.com", port: null });
  const cases: [Connection, string, boolean][] = [
    [dotcom, "https://github.com/o/r/pull/1", true],
    [dotcom, "https://GitHub.com/o/r/pull/1", true],
    [dotcom, "https://github.com:443/o/r/pull/1", true],
    [dotcom, "http://github.com/o/r/pull/1", false],
    [dotcom, "https://github.com.evil.test/o/r/pull/1", false],
    [dotcom, "https://evil.test/o/r/pull/1", false],
    [dotcom, "https://github.com:8443/o/r/pull/1", false],
    [dotcom, "https://user:pass@github.com/o/r/pull/1", false],
    [dotcom, "https://user@github.com/o/r/pull/1", false],
    [dotcom, "javascript:alert(1)", false],
    [dotcom, "-a /Applications/Calculator.app", false],
    [dotcom, "not a url", false],
    [http, "http://ghe.example.com:8080/o/r/pull/1", true],
    [http, "https://ghe.example.com:8080/o/r/pull/1", true],
    [http, "http://ghe.example.com/o/r/pull/1", false],
    [https, "https://ghe.example.com/o/r/pull/1", true],
    [https, "http://ghe.example.com/o/r/pull/1", false],
    [https, "https://ghe.example.com:8080/o/r/pull/1", false],
  ];
  for (const [connected, url, allowed] of cases) {
    expect([url, openableUrl(connected, url)]).toEqual([url, allowed]);
  }
});

test("the_refusal_names_the_schemes_the_server_allows", () => {
  expect(refusedUrlText(on({}))).toBe("Only an https:// address on github.com can be opened");
  expect(refusedUrlText(on({ useHttp: true, host: "ghe.example.com", port: 8080 }))).toBe(
    "Only an https:// or http:// address on ghe.example.com:8080 can be opened",
  );
});

// `GithubViewPullRequestsAction.actionPerformed`: `activateAndAwaitProject { viewList() }`, the
// window to the front on its list tab; the pull requests' tabs stay.
test("view_pull_requests_brings_the_window_to_the_front_on_its_list", () => {
  const asked: string[] = [];
  const stop = onWindowActivationRequested((id) => asked.push(id));
  connected(connection("origin", "github.com/octocat"));
  openPullRequestTab(7);
  expect(get(pullsStore).selected).toBe(7);

  viewPullRequestList();

  expect(get(pullsStore).selected).toBe(LIST_TAB);
  expect(get(pullsStore).tabs).toEqual([7]);
  expect(asked.at(-1)).toBe("pulls");
  stop();
});
