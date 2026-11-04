// WebKit applies the macOS substitutions in fields: `--` gives a dash, the first letter is capitalized
// and words get corrected. Branches, paths and messages are typed here, and IntelliJ touches nothing.
export function plainInputs(root: Document): () => void {
  const plain = (event: Event) => {
    const field = event.target;
    if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement)) return;
    field.setAttribute("spellcheck", "false");
    field.setAttribute("autocorrect", "off");
    field.setAttribute("autocapitalize", "off");
  };
  root.addEventListener("focusin", plain);
  return () => root.removeEventListener("focusin", plain);
}
