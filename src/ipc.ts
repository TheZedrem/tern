import { type Channel, invoke } from "@tauri-apps/api/core";
import type { PaneId } from "./boot/boot";

/** One message on a pane's output Channel: raw PTY bytes, or the final exit notice. */
export type PaneMessage = ArrayBuffer | { exit: number | null };

/** Mirrors `timing::TtfpReport`: ms since Rust `main()`, per boot mark. */
export type TtfpReport = Record<"config" | "pty" | "window" | "first_byte" | "rendered", number | null>;

/**
 * The IPC contract: every Tauri command with its argument and result types.
 * Adding a command in Rust means adding it here; `call` then type-checks
 * the name, the arguments and the result at every call site.
 */
type Commands = {
  pty_attach: { args: { id: PaneId; onOutput: Channel<PaneMessage> }; result: void };
  pty_write: { args: { id: PaneId; data: string }; result: void };
  pty_resize: { args: { id: PaneId; cols: number; rows: number }; result: void };
  pty_ack: { args: { id: PaneId; bytes: number }; result: void };
  report_first_prompt: { args: Record<string, never>; result: TtfpReport };
  app_quit: { args: Record<string, never>; result: void };
};

/** The only way the frontend talks to Rust. Rejects with Rust's error string. */
export function call<C extends keyof Commands>(
  command: C,
  args: Commands[C]["args"],
): Promise<Commands[C]["result"]> {
  return invoke(command, args);
}
