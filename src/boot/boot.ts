import type { ITheme } from "@xterm/xterm";

/** Mirrors `config::Config` in Rust (the T-001 subset). */
export type Config = {
  font: { family: string; size: number; line_height: number };
  scrollback: number;
  close_on_exit: "always" | "clean" | "never";
};

export type PaneId = number;

/** A boot pane is either running, or failed to spawn and shows why. */
export type BootPane = { id: PaneId } | { error: string; command: string };

/**
 * Everything the first paint needs, injected by Rust as `window.__BOOT__`
 * before any page script runs (spec 0001). Reading it costs no IPC.
 */
export type Boot = {
  config: Config;
  theme: ITheme;
  panes: BootPane[];
  notices: string[];
  bench: boolean;
  webgl: boolean;
};

declare global {
  interface Window {
    __BOOT__: Boot;
  }
}

export const boot = window.__BOOT__;

/**
 * Runs `fn` once the page is idle. WebKitGTK has no `requestIdleCallback`,
 * so it falls back to a short timeout there.
 */
export function whenIdle(fn: () => void): void {
  if ("requestIdleCallback" in window) requestIdleCallback(() => fn(), { timeout: 1000 });
  else setTimeout(fn, 100);
}
