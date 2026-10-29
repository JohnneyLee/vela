import { createSignal, onCleanup, onMount } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./tokens.css";
import "./bar.css";

const TIP_KEY = "vela.bar.tipSeen";

function Bar() {
  const [showTip, setShowTip] = createSignal(false);
  const [near, setNear] = createSignal(false);
  let ignoreUntil = 0;

  function openPanel() {
    if (Date.now() < ignoreUntil) return;
    // 只取词并打开结果卡，不自动跑译/释/润——等用户在卡片上点四个字之一。
    invoke("bar_pick", { action: "choose" });
    if (!localStorage.getItem(TIP_KEY)) {
      localStorage.setItem(TIP_KEY, "1");
      setShowTip(false);
    }
  }

  onMount(() => {
    const unsubs = [];
    listen("bar:show", () => {
      ignoreUntil = Date.now() + 180;
      invoke("bar_layout", { mode: "collapsed" });
      if (!localStorage.getItem(TIP_KEY)) setShowTip(true);
    }).then((u) => unsubs.push(u));

    listen("bar:hide", () => setShowTip(false)).then((u) => unsubs.push(u));

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
    <div
      class="bar"
      classList={{ near: near() }}
      onMouseEnter={() => setNear(true)}
      onMouseLeave={() => setNear(false)}
    >
      <button
        type="button"
        class="bar-glyph"
        aria-label="打开 Vela，选择功能"
        title="打开后选择：译 / 释 / 润 / 读"
        onClick={openPanel}
      >
        <span class="bar-glyph-mark" aria-hidden="true">
          V
        </span>
      </button>
      {showTip() && (
        <div class="bar-tip" role="status">
          点开后选：译 / 释 / 润 / 读
        </div>
      )}
    </div>
  );
}

export default Bar;
