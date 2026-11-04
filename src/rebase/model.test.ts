import { describe, expect, test } from "vitest";
import { RebaseModel, shortcutAction, type RebaseCommit } from "./model";

const commit = (name: string, action = "pick"): RebaseCommit => ({
  action,
  hash: name.toLowerCase().repeat(7),
  subject: name,
  full: name.toLowerCase().repeat(40),
  message: `${name}\n\nbody of ${name}`,
});

// A, B, C and D, from the oldest to the most recent, like git's list.
const four = () => new RebaseModel(["A", "B", "C", "D"].map((name) => commit(name)));

const lines = (model: RebaseModel) =>
  model.entries().entries.map(({ action, subject }) => `${action} ${subject}`);

describe("RebaseModel", () => {
  test("squash_unites_with_the_previous_one_that_is_kept", () => {
    const model = four();
    // With a single row, the root is the previous one that keeps its commit.
    expect(model.enabled("squash", [2])).toBe(true);
    const root = model.squash([2]);
    expect(root).toBe(1);
    expect(lines(model)).toEqual(["pick A", "reword B", "fixup C", "pick D"]);

    // A dropped previous one does not count: it skips to the one that is kept.
    const other = four();
    other.drop([1]);
    expect(other.squash([2])).toBe(0);
    expect(lines(other)).toEqual(["reword A", "fixup C", "drop B", "pick D"]);
    const rows = other.rows();
    expect(rows.map(({ node }) => node)).toEqual(["double", "none", "none", "simple"]);
    expect(rows[1].child).toEqual({ first: true, last: true });
    expect(rows[2].dropped).toBe(true);
  });

  test("squash_on_the_first_row_disabled", () => {
    const model = four();
    expect(model.enabled("squash", [0])).toBe(false);
    expect(model.enabled("fixup", [0])).toBe(false);
    // Without a selection, none.
    for (const action of ["pick", "edit", "reword", "squash", "fixup", "drop"] as const) {
      expect(model.enabled(action, [])).toBe(false);
    }
    // While a message is being edited, neither.
    expect(model.enabled("drop", [1], true)).toBe(false);
    expect(model.enabled("drop", [1])).toBe(true);
    // Already united into the same group, they cannot be united again.
    model.fixup([0, 1]);
    expect(model.enabled("fixup", [0, 1])).toBe(false);
    expect(model.enabled("fixup", [0, 1, 2])).toBe(true);
  });

  test("squash_marks_the_root_as_reword_with_the_united_messages", () => {
    const model = four();
    const root = model.squash([0, 2]);

    // `unite`: the rest of the group goes right behind the root.
    expect(root).toBe(0);
    expect(lines(model)).toEqual(["reword A", "fixup C", "pick B", "pick D"]);
    const united = "A\n\nbody of A\n\n\nC\n\nbody of C";
    expect(model.message(0)).toBe(united);
    expect(model.entries().messages).toEqual([{ hash: "aaaaaaa", message: united }]);
    expect(model.rows()[0]).toMatchObject({ node: "double", reworded: true, subject: "A" });

    // One more into the group: the group's message, already edited, and the new one's.
    model.reword(0, "AC");
    model.squash([0, 3]);
    expect(lines(model)).toEqual(["reword A", "fixup C", "fixup D", "pick B"]);
    expect(model.message(0)).toBe("AC\n\n\nD\n\nbody of D");

    // Fixup unites without touching the message.
    const fixed = four();
    fixed.fixup([1, 3]);
    expect(lines(fixed)).toEqual(["pick A", "pick B", "fixup D", "pick C"]);
    expect(fixed.entries().messages).toEqual([]);
  });

  test("reword_equal_to_the_original_goes_back_to_pick", () => {
    const model = four();
    model.reword(1, "B new\n\nbody");
    expect(lines(model)).toEqual(["pick A", "reword B", "pick C", "pick D"]);
    expect(model.rows()[1]).toMatchObject({ reworded: true, subject: "B new" });
    expect(model.message(1)).toBe("B new\n\nbody");

    model.reword(1, "B\n\nbody of B");
    expect(lines(model)).toEqual(["pick A", "pick B", "pick C", "pick D"]);
    expect(model.entries().messages).toEqual([]);
    // A child of a group cannot be edited.
    model.fixup([1, 2]);
    expect(model.enabled("reword", [2])).toBe(false);
    expect(model.enabled("reword", [1])).toBe(true);
    expect(model.enabled("reword", [0, 1])).toBe(false);
  });

  test("initial_drop_moves_to_the_end", () => {
    const model = new RebaseModel([
      commit("A", "drop"),
      commit("B"),
      commit("C", "fixup"),
      commit("D"),
    ]);

    expect(lines(model)).toEqual(["pick B", "fixup C", "pick D", "drop A"]);
    expect(model.rows().map(({ subject }) => subject)).toEqual(["B", "C", "D", "A"]);

    // What is dropped afterwards stays where it is, and a dropped group is undone.
    const other = four();
    other.fixup([0, 1]);
    other.drop([0]);
    expect(lines(other)).toEqual(["drop A", "drop B", "pick C", "pick D"]);
  });

  test("undo_keeps_ten_states", () => {
    const model = four();
    expect(model.changes).toBe(0);
    // Twelve changes: the first two can no longer be undone.
    for (let index = 0; index < 12; index += 1) {
      if (index % 2 === 0) model.drop([0]);
      else model.pick([0]);
    }
    expect(model.changes).toBe(12);
    const states: string[][] = [];
    for (let index = 0; index < 12; index += 1) {
      model.undo();
      states.push(lines(model));
    }
    // Nine steps back, and it stays there.
    expect(states[8]).toEqual(states[11]);
    expect(states[7]).not.toEqual(states[8]);
    expect(states[8]).toEqual(["drop A", "pick B", "pick C", "pick D"]);
    model.redo();
    expect(lines(model)).toEqual(["pick A", "pick B", "pick C", "pick D"]);

    // A change after undoing discards what was ahead.
    model.edit([3]);
    model.redo();
    expect(lines(model)).toEqual(["pick A", "pick B", "pick C", "edit D"]);

    // Reset goes back to the starting list, and can also be undone.
    model.reset();
    expect(lines(model)).toEqual(["pick A", "pick B", "pick C", "pick D"]);
    model.undo();
    expect(lines(model)).toEqual(["pick A", "pick B", "pick C", "edit D"]);
  });

  test("move_up_and_down_with_the_groups", () => {
    const model = four();
    expect(model.enabled("moveUp", [0])).toBe(false);
    expect(model.enabled("moveDown", [3])).toBe(false);
    expect(model.moveUp([2, 3])).toEqual([1, 2]);
    expect(lines(model)).toEqual(["pick A", "pick C", "pick D", "pick B"]);
    expect(model.moveDown([0])).toEqual([1]);
    expect(lines(model)).toEqual(["pick C", "pick A", "pick D", "pick B"]);
    // Each moved row is a change that is undone.
    expect(model.changes).toBe(3);

    // The root moves with its group.
    const grouped = four();
    grouped.fixup([1, 2]);
    grouped.moveDown([1]);
    expect(lines(grouped)).toEqual(["pick A", "pick D", "pick B", "fixup C"]);
  });

  test("shortcuts_alt_p_e_d_r_s_f", () => {
    const key = (code: string, extra: Partial<KeyboardEvent> = {}) =>
      shortcutAction({ key: "", code, altKey: false, metaKey: false, ctrlKey: false, shiftKey: false, ...extra });
    // `ChangeEntryStateSimpleAction`: ⌥ with the initial of the command.
    expect(key("KeyP", { altKey: true, key: "π" })).toBe("pick");
    expect(key("KeyE", { altKey: true })).toBe("edit");
    expect(key("KeyD", { altKey: true })).toBe("drop");
    expect(key("KeyR", { altKey: true })).toBe("reword");
    expect(key("KeyS", { altKey: true })).toBe("squash");
    expect(key("KeyF", { altKey: true })).toBe("fixup");
    expect(key("KeyP")).toBeNull();
    expect(key("KeyP", { altKey: true, metaKey: true })).toBeNull();
    // Those of its action in the keymap: `$Delete`, `Git.Reword.Commit`, `$Undo` and `$Redo`, and those of
    // `CommonShortcuts.MOVE_UP` and `MOVE_DOWN`.
    expect(key("Backspace", { key: "Backspace" })).toBe("drop");
    expect(key("Delete", { key: "Delete" })).toBe("drop");
    expect(key("Backspace", { key: "Backspace", metaKey: true })).toBe("drop");
    expect(key("F2", { key: "F2" })).toBe("reword");
    expect(key("F6", { key: "F6", shiftKey: true })).toBe("reword");
    expect(key("ArrowUp", { key: "ArrowUp", altKey: true })).toBe("moveUp");
    expect(key("ArrowDown", { key: "ArrowDown", altKey: true })).toBe("moveDown");
    expect(key("ArrowUp", { key: "ArrowUp" })).toBeNull();
    expect(key("KeyZ", { key: "z", metaKey: true })).toBe("undo");
    expect(key("KeyZ", { key: "z", metaKey: true, shiftKey: true })).toBe("redo");
  });
});
