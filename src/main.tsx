// The boot path (spec 0001). Everything imported here, directly or
// transitively, lands in the entry chunk that every launch parses, and
// `pnpm build` enforces its budget. Features are reached via `import()`.
import "./styles.css";
import { render } from "solid-js/web";
import { App } from "./App";
import { boot } from "./boot/boot";

// Match the window background Rust already set, so there is no flash
// between the native background and the first xterm frame.
document.documentElement.style.background = boot.theme.background ?? "";

const root = document.getElementById("root");
if (!root) throw new Error("index.html is missing #root");
render(() => <App />, root);
