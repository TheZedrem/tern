import { createSignal, For, onCleanup } from "solid-js";

const AUTO_DISMISS_MS = 10_000;

/**
 * Notice toasts (architecture §Cross-cutting rules): plain text, stacked
 * bottom-right, dismissed by click or after 10 s. They never take keyboard
 * focus, so typing keeps going to the pane. Lazy: loaded only when a notice
 * exists.
 */
export default function Notices(props: { notices: string[] }) {
  const [shown, setShown] = createSignal(props.notices);
  const timer = setTimeout(() => setShown([]), AUTO_DISMISS_MS);
  onCleanup(() => clearTimeout(timer));

  return (
    <div class="fixed right-3 bottom-3 flex max-w-md flex-col gap-2">
      <For each={shown()}>
        {(notice) => (
          <button
            type="button"
            tabindex="-1"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setShown((all) => all.filter((n) => n !== notice))}
            class="rounded-md bg-neutral-800/95 px-3 py-2 text-left text-sm text-neutral-100 shadow-lg ring-1 ring-white/10"
          >
            {notice}
          </button>
        )}
      </For>
    </div>
  );
}
