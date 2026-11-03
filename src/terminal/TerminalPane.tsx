import { Channel } from "@tauri-apps/api/core";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { onCleanup, onMount } from "solid-js";
import { type BootPane, boot, whenIdle } from "../boot/boot";
import { call, type PaneMessage } from "../ipc";

/** Acks are batched: sent once this many bytes are consumed, or on the next tick. */
const ACK_BATCH = 32 * 1024;

type Props = {
  pane: BootPane;
  /** Called once, after the first output (or spawn error) is on screen. */
  onFirstRender: () => void;
};

/**
 * One terminal pane (spec 0002): an xterm view bound to a Rust PTY through
 * a binary Channel. Output is acked after xterm consumes it, which is what
 * lets Rust apply backpressure. Solid runs this once; nothing here is
 * reactive except the DOM ref.
 */
export function TerminalPane(props: Props) {
  let container!: HTMLDivElement;

  onMount(() => {
    const { font, scrollback } = boot.config;
    const term = new Terminal({
      fontFamily: font.family,
      fontSize: font.size,
      lineHeight: font.line_height,
      scrollback,
      theme: boot.theme,
      allowProposedApi: false,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(container);
    fit.fit();

    // The first frame renders on the DOM renderer; WebGL init (~50 ms) is
    // deferred until after it, which keeps that time off the TTFP.
    const firstRender = () => {
      const sub = term.onRender(() => {
        sub.dispose();
        props.onFirstRender();
        if (boot.webgl) whenIdle(() => loadWebgl(term));
      });
    };

    const pane = props.pane;
    if (!("id" in pane)) {
      term.write(`\x1b[31mCould not start ${pane.command}: ${pane.error}\x1b[0m\r\n`, firstRender);
      onCleanup(() => term.dispose());
      return;
    }
    const id = pane.id;

    let unacked = 0;
    let ackScheduled = false;
    const flushAck = () => {
      ackScheduled = false;
      if (unacked === 0) return;
      void call("pty_ack", { id, bytes: unacked });
      unacked = 0;
    };
    const consumed = (bytes: number) => {
      unacked += bytes;
      if (unacked >= ACK_BATCH) flushAck();
      else if (!ackScheduled) {
        ackScheduled = true;
        // Not requestAnimationFrame: it pauses in a hidden window, which
        // would stall the shell behind backpressure.
        setTimeout(flushAck, 0);
      }
    };

    let sawOutput = false;
    const channel = new Channel<PaneMessage>();
    channel.onmessage = (msg) => {
      if (msg instanceof ArrayBuffer) {
        const bytes = new Uint8Array(msg);
        term.write(bytes, () => {
          consumed(bytes.length);
          if (!sawOutput) {
            sawOutput = true;
            firstRender();
          }
        });
      } else {
        onExit(term, msg.exit);
      }
    };
    void call("pty_attach", { id, onOutput: channel });

    term.onData((data) => void call("pty_write", { id, data }));

    let sent = { cols: 0, rows: 0 };
    const syncSize = () => {
      fit.fit();
      if (term.cols === sent.cols && term.rows === sent.rows) return;
      sent = { cols: term.cols, rows: term.rows };
      void call("pty_resize", { id, ...sent });
    };
    syncSize();

    // At most one fit per animation frame, however often the container resizes.
    let frame = 0;
    const observer = new ResizeObserver(() => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        syncSize();
      });
    });
    observer.observe(container);
    term.focus();

    onCleanup(() => {
      observer.disconnect();
      term.dispose();
    });
  });

  return <div ref={container} class="h-full w-full p-1" />;
}

/**
 * Switches to WebGL after the first frame; the DOM renderer is the mandatory
 * fallback (architecture §Key decisions). Disposing the addon on context
 * loss switches xterm back to the DOM renderer without losing the buffer.
 */
function loadWebgl(term: Terminal): void {
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => webgl.dispose());
    term.loadAddon(webgl);
  } catch {
    // WebGL unavailable (common on WebKitGTK): stay on the DOM renderer.
  }
}

/** Shows the exit notice, then closes per `close_on_exit` (spec 0002 flow 8). */
function onExit(term: Terminal, code: number | null): void {
  const shown = code ?? "unknown (killed)";
  term.write(`\r\n\x1b[2m[process exited with code ${shown}]\x1b[0m\r\n`, () => {
    if (boot.bench) return; // bench quits from report_first_prompt
    const policy = boot.config.close_on_exit;
    // One pane per window until T-003, so closing the pane quits the app.
    if (policy === "always" || (policy === "clean" && code === 0)) void call("app_quit", {});
  });
}
