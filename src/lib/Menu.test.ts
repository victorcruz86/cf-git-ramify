import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, expect, test, vi } from "vitest";
import Menu from "./Menu.svelte";

type MenuItem =
  | {
      label: string;
      run?: () => void;
      submenu?: MenuItem[];
      star?: { on: boolean; toggle: () => void };
    }
  | { header: string }
  | null;

const labels = (menu: HTMLElement) =>
  within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));

const menus = () => screen.getAllByRole("menu");

function show(items: MenuItem[], props: Record<string, unknown> = {}) {
  const onclose = vi.fn();
  render(Menu, { x: 0, y: 0, items, label: "Test", onclose, ...props });
  return onclose;
}

const groups = (run = () => {}): MenuItem[] => [
  { label: "one", run },
  {
    label: "Local",
    submenu: [
      { label: "main", run },
      { label: "side", run },
    ],
  },
];

const key = (element: Element, name: string) => fireEvent.keyDown(element, { key: name });

afterEach(cleanup);

test("enter_or_space_without_a_highlighted_row_do_nothing", async () => {
  const run = vi.fn();
  const onclose = show(groups(run));
  const root = screen.getByRole("menu", { name: "Test" });
  const errors = vi.fn();
  window.addEventListener("error", errors);

  await key(root, "Enter");
  await key(root, " ");

  window.removeEventListener("error", errors);
  expect(errors).not.toHaveBeenCalled();
  expect(run).not.toHaveBeenCalled();
  expect(onclose).not.toHaveBeenCalled();
});

test("without_a_submenu_an_item_runs", async () => {
  const run = vi.fn();
  const onclose = show(groups(run));

  await fireEvent.click(screen.getByRole("menuitem", { name: "one" }));

  expect(onclose).toHaveBeenCalledOnce();
  await waitFor(() => expect(run).toHaveBeenCalledOnce());
});

test("header_with_text", () => {
  show([{ header: "Recent" }, { label: "one", run: () => {} }]);

  expect(screen.getByRole("separator", { name: "Recent" }).textContent).toContain("Recent");
});

test("submenu_opens_on_hover", async () => {
  show(groups());
  expect(menus()).toHaveLength(1);

  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Local" }));

  expect(labels(screen.getByRole("menu", { name: "Local" }))).toEqual(["main", "side"]);
  // Without stealing focus: the keyboard stays on the menu above.
  expect(document.activeElement).toBe(screen.getByRole("menu", { name: "Test" }));

  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "one" }));

  expect(menus()).toHaveLength(1);
});

test("right_opens_and_left_goes_back", async () => {
  const onclose = show(groups());
  const root = screen.getByRole("menu", { name: "Test" });
  await key(root, "ArrowDown");
  await key(root, "ArrowDown");

  await key(root, "ArrowRight");

  const child = screen.getByRole("menu", { name: "Local" });
  expect(document.activeElement).toBe(child);
  expect(within(child).getByRole("menuitem", { name: "main" }).className).toContain("current");

  await key(child, "ArrowLeft");

  expect(menus()).toHaveLength(1);
  expect(document.activeElement).toBe(root);
  expect(onclose).not.toHaveBeenCalled();
});

test("enter_opens_the_submenu_without_running", async () => {
  const onclose = show(groups());
  const root = screen.getByRole("menu", { name: "Test" });
  await key(root, "ArrowUp");

  await key(root, "Enter");

  expect(screen.getByRole("menu", { name: "Local" })).toBeTruthy();
  expect(onclose).not.toHaveBeenCalled();
});

test("escape_in_the_submenu_only_closes_it", async () => {
  const onclose = show(groups());
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Local" }));
  const child = screen.getByRole("menu", { name: "Local" });

  await key(child, "Escape");

  expect(menus()).toHaveLength(1);
  expect(onclose).not.toHaveBeenCalled();
});

test("choosing_in_the_submenu_closes_everything", async () => {
  const run = vi.fn();
  const onclose = show(groups(run));
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Local" }));

  await fireEvent.click(screen.getByRole("menuitem", { name: "side" }));

  expect(onclose).toHaveBeenCalledOnce();
  await waitFor(() => expect(run).toHaveBeenCalledOnce());
});

test("click_inside_the_submenu_does_not_count_as_outside", async () => {
  const onclose = show(groups());
  await fireEvent.pointerEnter(screen.getByRole("menuitem", { name: "Local" }));

  await fireEvent.pointerDown(screen.getByRole("menuitem", { name: "side" }));
  expect(onclose).not.toHaveBeenCalled();

  await fireEvent.pointerDown(document.body);
  expect(onclose).toHaveBeenCalledOnce();
});

test("without_speed_search_the_letters_do_nothing", async () => {
  show(groups());
  const root = screen.getByRole("menu", { name: "Test" });

  await key(root, "m");

  expect(labels(root)).toEqual(["one", "Local"]);
});

test("speed_search_switches_to_the_search_rows", async () => {
  const typing: MenuItem[] = [
    { label: "one", run: () => {} },
    { header: "Local" },
    { label: "main", run: () => {} },
    { label: "side", run: () => {} },
    { header: "origin" },
    { label: "origin/side", run: () => {} },
  ];
  show(groups(), { typing });
  const root = screen.getByRole("menu", { name: "Test" });

  await key(root, "m");

  expect(labels(root)).toEqual(["main"]);
  expect(screen.getByRole("separator", { name: "Local" })).toBeTruthy();
  expect(screen.queryByRole("separator", { name: "origin" })).toBeNull();
  expect(root.textContent).toContain("m");

  await key(root, "Backspace");

  expect(labels(root)).toEqual(["one", "Local"]);
});

test("speed_search_enter_chooses_the_first", async () => {
  const run = vi.fn();
  const typing: MenuItem[] = [{ label: "main", run }];
  const onclose = show(groups(), { typing });
  const root = screen.getByRole("menu", { name: "Test" });

  await key(root, "m");
  await key(root, "Enter");

  expect(onclose).toHaveBeenCalledOnce();
  await waitFor(() => expect(run).toHaveBeenCalledOnce());
});

test("escape_with_a_search_clears_it_first", async () => {
  const typing: MenuItem[] = [{ label: "main", run: () => {} }];
  const onclose = show(groups(), { typing });
  const root = screen.getByRole("menu", { name: "Test" });
  await key(root, "m");

  await key(root, "Escape");

  expect(onclose).not.toHaveBeenCalled();
  expect(labels(root)).toEqual(["one", "Local"]);
});

test("numbered_menu_chooses_with_the_number", async () => {
  const first = vi.fn();
  const second = vi.fn();
  const onclose = show([{ header: "Title" }, { label: "one", run: first }, { label: "two", run: second }], {
    numbered: true,
  });
  const root = screen.getByRole("menu", { name: "Test" });
  const rows = within(root).getAllByRole("menuitem");
  expect(rows[0].textContent).toMatch(/1\.\s+one/);
  expect(rows[1].textContent).toMatch(/2\.\s+two/);

  await key(root, "3");
  expect(onclose).not.toHaveBeenCalled();

  await key(root, "2");

  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(second).toHaveBeenCalled());
  expect(first).not.toHaveBeenCalled();
});

test("without_numbering_the_digits_do_not_choose", async () => {
  const run = vi.fn();
  const onclose = show([{ label: "one", run }]);

  await key(screen.getByRole("menu", { name: "Test" }), "1");

  expect(onclose).not.toHaveBeenCalled();
});

test("the_star_toggles_without_choosing_the_row_or_closing", async () => {
  const run = vi.fn();
  const toggle = vi.fn();
  const onclose = show([{ label: "side", run, star: { on: false, toggle } }]);
  const row = screen.getByRole("menuitem", { name: "side" });
  const star = row.querySelector("[data-star]")!;
  expect(star.getAttribute("data-star")).toBe("off");

  await fireEvent.click(star);

  expect(toggle).toHaveBeenCalledTimes(1);
  expect(run).not.toHaveBeenCalled();
  expect(onclose).not.toHaveBeenCalled();
  expect(star.getAttribute("data-star")).toBe("on");
});

test("click_on_the_starred_row_chooses_it", async () => {
  const run = vi.fn();
  const onclose = show([{ label: "side", run, star: { on: true, toggle: vi.fn() } }]);

  await fireEvent.click(screen.getByRole("menuitem", { name: "side" }));

  expect(onclose).toHaveBeenCalled();
  await waitFor(() => expect(run).toHaveBeenCalled());
});
