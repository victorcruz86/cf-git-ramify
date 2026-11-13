// A text field whose `<datalist>` suggestions show only while the user types, like IntelliJ's
// `ComboBoxWithAutoCompletion`. WebKit opens a datalist as soon as its input gets the focus, and
// the dialogs focus their branch field when they open: the list covered their buttons.
//
// The `list` attribute is added on the first keystroke and removed when the field loses the
// focus. The field keeps the combobox role it has with a datalist.
export function suggestWhileTyping(input: HTMLInputElement, list: string) {
  let id = list;
  input.removeAttribute("list");
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-autocomplete", "list");
  const typing = () => {
    if (input.getAttribute("list") !== id) input.setAttribute("list", id);
  };
  const left = () => input.removeAttribute("list");
  // `beforeinput` comes before the value changes, so the keystroke that adds the list already
  // shows its suggestions; `input` covers what does not send it.
  input.addEventListener("beforeinput", typing);
  input.addEventListener("input", typing);
  input.addEventListener("blur", left);
  return {
    update(next: string) {
      id = next;
      if (input.hasAttribute("list")) input.setAttribute("list", id);
    },
    destroy() {
      input.removeEventListener("beforeinput", typing);
      input.removeEventListener("input", typing);
      input.removeEventListener("blur", left);
    },
  };
}
