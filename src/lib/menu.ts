export interface MenuAction {
  label: string;
  shortcut?: string;
  disabled?: boolean;
  // Why it is disabled: `PushActionBase.getDescription`.
  title?: string;
  checked?: boolean;
  run?: () => void;
  // Opens on hover, with Right or with Enter, instead of running `run`.
  submenu?: MenuItem[];
  // `PopupListElementRendererWithIcon`: the star of the row. Clicking it does not close the menu.
  star?: { on: boolean; toggle: () => void };
}

// A separator with text, like `Separator.create`.
export interface MenuHeader {
  header: string;
}

// A separator is written as `null`.
export type MenuItem = MenuAction | MenuHeader | null;
