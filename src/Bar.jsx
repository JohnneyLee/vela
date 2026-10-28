import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./tokens.css";
import "./bar.css";

const ACTIONS = [
  { id: "translate", label: "译" },
  { id: "explain", label: "释" },
  { id: "polish", label: "润" },
  { id: "speak", label: "读" },
];

function Bar() {
  const [suggested, setSuggested] = createSignal("translate");

  function pick(action) {
    invoke("bar_pick", { action });
  }

  onMount(() => {
    const unsubs = [];
    listen("bar:show", (event) => {
      const p = event.payload;
      if (p?.suggested) setSuggested(p.suggested);
    }).then((u) => unsubs.push(u));

    const onKey = (e) => {
      if (e.key === "Escape") invoke("bar_hide");
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      unsubs.forEach((u) => u());
    });
  });

  return (
    <div class="bar">
      {ACTIONS.map((a) => (
        <button
          type="button"
          class="bar-btn"
          classList={{ suggested: suggested() === a.id }}
          aria-label={a.label}
          onClick={() => pick(a.id)}
        >
          {a.label}
        </button>
      ))}
    </div>
  );
}

export default Bar;
