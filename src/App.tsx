import { createSignal, For, lazy, Show } from "solid-js";
import { boot, whenIdle } from "./boot/boot";
import { call } from "./ipc";
import { TerminalPane } from "./terminal/TerminalPane";

const Notices = lazy(() => import("./features/notices/Notices"));

/**
 * The window: the boot panes, the dev TTFP badge, and boot notices. Once the
 * first prompt is on screen it reports TTFP, and only then, at idle, does
 * any deferred work start (spec 0001, step 7).
 */
export function App() {
  const [ttfp, setTtfp] = createSignal<number | null>(null);
  const [showNotices, setShowNotices] = createSignal(false);

  let reported = false;
  const onFirstRender = async () => {
    if (reported) return;
    reported = true;
    const report = await call("report_first_prompt", {});
    setTtfp(report.rendered);
    whenIdle(() => setShowNotices(boot.notices.length > 0));
  };

  return (
    <div class="h-full w-full">
      <For each={boot.panes}>{(pane) => <TerminalPane pane={pane} onFirstRender={onFirstRender} />}</For>
      <Show when={import.meta.env.DEV && ttfp()}>
        {(ms) => (
          <div class="pointer-events-none fixed top-2 right-3 rounded bg-black/60 px-2 py-0.5 font-mono text-xs text-white">
            TTFP {Math.round(ms())} ms
          </div>
        )}
      </Show>
      <Show when={showNotices()}>
        <Notices notices={boot.notices} />
      </Show>
    </div>
  );
}
