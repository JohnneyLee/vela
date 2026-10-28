import { createSignal, onCleanup, onMount, Show, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { OcrBlock } from "./OcrBlock";
import "./tokens.css";
import "./overlay.css";

// 钉住的截图翻译结果（O6）：独立窗口，可拖动、缩放、调透明度、穿透点击，
// 按住 Ctrl 临时显示原图，Esc 或 × 关闭并清空内存。

function Pinned() {
  const [data, setData] = createSignal(null);
  const [scale, setScale] = createSignal(1);
  const [opacity, setOpacity] = createSignal(1);
  const [clickThrough, setClickThrough] = createSignal(false);
  const [showOriginal, setShowOriginal] = createSignal(false);
  let canvasRef;

  const id = Number(window.location.hash.split("/").filter(Boolean).pop() || 0);

  const dpr = () => window.devicePixelRatio || 1;

  function paintCanvas(d) {
    const canvas = canvasRef;
    canvas.width = d.width;
    canvas.height = d.height;
    canvas.style.width = `${d.width / dpr()}px`;
    canvas.style.height = `${d.height / dpr()}px`;
    const bytes = Uint8ClampedArray.from(atob(d.b64), (c) => c.charCodeAt(0));
    const ctx = canvas.getContext("2d");
    ctx.putImageData(new ImageData(bytes, d.width, d.height), 0, 0);
  }

  onMount(() => {
    invoke("get_pinned_result", { id })
      .then((d) => {
        setData(d);
        paintCanvas(d);
      })
      .catch(() => invoke("close_pinned", { id }));

    const onKey = (e) => {
      if (e.key === "Escape") invoke("close_pinned", { id });
    };
    // 按住 Ctrl 临时显示原图（隐藏译文块）。
    const onKeyDown = (e) => {
      if (e.key === "Control") setShowOriginal(true);
    };
    const onKeyUp = (e) => {
      if (e.key === "Control") setShowOriginal(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    });
  });

  function toggleClickThrough() {
    const next = !clickThrough();
    setClickThrough(next);
    getCurrentWindow().setIgnoreCursorEvents(next).catch(() => {});
  }

  function changeScale(delta) {
    const next = Math.min(2, Math.max(0.5, Math.round((scale() + delta) * 10) / 10));
    setScale(next);
    const d = data();
    if (!d) return;
    getCurrentWindow()
      .setSize(new PhysicalSize(Math.round(d.width * next), Math.round(d.height * next)))
      .catch(() => {});
  }

  return (
    <div class="pinned-root">
      <div
        class="pinned-stage"
        style={{
          transform: `scale(${scale()})`,
          "transform-origin": "top left",
          opacity: opacity(),
        }}
      >
        <canvas ref={canvasRef} class="overlay-canvas" />
        <Show when={!showOriginal() && data()}>
          <For each={data().blocks.filter((b) => b.translated)}>
            {(b) => <OcrBlock block={b} />}
          </For>
        </Show>
      </div>
      <div class="pinned-toolbar">
        <div class="pinned-drag" data-tauri-drag-region />
        <button type="button" class="pinned-btn" onClick={() => changeScale(-0.1)}>−</button>
        <span class="pinned-scale">{Math.round(scale() * 100)}%</span>
        <button type="button" class="pinned-btn" onClick={() => changeScale(0.1)}>＋</button>
        <button
          type="button"
          class="pinned-btn"
          classList={{ on: clickThrough() }}
          onClick={toggleClickThrough}
          title="穿透点击"
        >
          穿透
        </button>
        <input
          type="range"
          class="pinned-opacity"
          min="0.3"
          max="1"
          step="0.05"
          value={opacity()}
          onInput={(e) => setOpacity(Number(e.currentTarget.value))}
          title="透明度"
        />
        <button type="button" class="pinned-btn" onClick={() => invoke("close_pinned", { id })}>×</button>
      </div>
    </div>
  );
}

export default Pinned;
