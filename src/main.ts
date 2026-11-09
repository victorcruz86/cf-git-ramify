import { mount } from "svelte";
import "./lib/controls.css";
import { plainInputs } from "./lib/plainInputs";
import { applyTheme } from "./lib/theme";

applyTheme(document.documentElement);
plainInputs(document);
const target = document.getElementById("app")!;

// The windows come from the same bundle, and each one loads only its own part: the diff editor
// does not delay the startup of the main one.
const kind = new URLSearchParams(location.search).get("window");
if (kind === "diff") {
  import("./diff/window").then(({ mountDiffWindow }) => mountDiffWindow(target));
} else if (kind === "merge") {
  import("./merge/window").then(({ mountMergeWindow }) => mountMergeWindow(target));
} else if (kind === "viewer") {
  import("./viewer/window").then(({ mountViewerWindow }) => mountViewerWindow(target));
} else if (kind === "selection-history") {
  import("./history/window").then(({ mountSelectionHistoryWindow }) =>
    mountSelectionHistoryWindow(target),
  );
} else if (import.meta.env.VITE_RAMA_SMOKE) {
  import("./perf/smoke").then(({ runSmoke }) => runSmoke());
} else {
  import("./App.svelte").then(({ default: App }) => mount(App, { target }));
}
