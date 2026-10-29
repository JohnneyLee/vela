import { createSignal, onCleanup, onMount, Show, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { OcrBlock, OcrOutline } from "./OcrBlock";
import "./tokens.css";
import "./overlay.css";

// 钉住：顶栏 + 原图 + 附近译文（优先右侧）。

function Pinned() {
  const [data, setData] = createSignal(null);
  const [scale, setScale] = createSignal(1);
  const [opacity, setOpacity] = createSignal(1);
  const [clickThrough, setClickThrough] = createSignal(false);
  const [showOriginal, setShowOriginal] = createSignal(false);
  let bodyRef;

  const id = Number(window.location.hash.split("/").filter(Boolean).pop() || 0);
  const dpr = () => window.devicePixelRatio || 1;
  const CHROME = 28;
  const CARDS_MIN = 220;

  function placeSide(d) {
    if (!d) return true;
    return d.width / Math.max(1, d.height) < 2.2;
  }

  function fitWindow() {
    const d = data();
    if (!d || !bodyRef) return;
    const s = scale();
    const side = placeSide(d);
    const shotW = d.width / dpr();
    const shotH = d.height / dpr();
    const cards = bodyRef.querySelector(".ocr-cards");
    const cardsW = cards ? Math.max(cards.scrollWidth, CARDS_MIN) : CARDS_MIN;
    const cardsH = cards ? cards.scrollHeight : 0;
    const w = (side ? shotW + cardsW : Math.max(shotW, cardsW)) * s;
    const h = CHROME + (side ? Math.max(shotH, cardsH) : shotH + cardsH) * s;
    getCurrentWindow()
      .setSize(new PhysicalSize(Math.ceil(w * dpr()), Math.ceil(h * dpr())))
      .catch(() => {});
  }

  function doClose() {
    invoke("close_pinned", { id }).catch(() => {});
    getCurrentWindow().close().catch(() => {});
  }

  onMount(() => {
    invoke("get_pinned_result", { id })
      .then((d) => {
        setData(d);
        requestAnimationFrame(() => fitWindow());
      })
      .catch(() => doClose());

    const onKey = (e) => {
      if (e.key === "Escape") doClose();
    };
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
    requestAnimationFrame(() => fitWindow());
  }

  function beginDrag(e) {
    if (e.button !== 0) return;
    if (e.target.closest("button, input, .pinned-no-drag")) return;
    getCurrentWindow().startDragging();
  }

  const translated = () => (data()?.blocks || []).filter((b) => b.translated);

  return (
    <div class="pinned-root" onMouseDown={beginDrag}>
      <div class="pinned-toolbar">
        <span class="pinned-drag-hint" data-tauri-drag-region title="按住拖动">
          ⋮⋮
        </span>
        <Show when={data()?.engineLabel}>
          <span class="ocr-engine-badge pinned-engine">{data().engineLabel}</span>
        </Show>
        <div class="pinned-no-drag pinned-controls">
          <button type="button" class="pinned-btn" onClick={() => changeScale(-0.1)}>−</button>
          <span class="pinned-scale">{Math.round(scale() * 100)}%</span>
          <button type="button" class="pinned-btn" onClick={() => changeScale(0.1)}>＋</button>
          <button
            type="button"
            class="pinned-btn"
            classList={{ on: clickThrough() }}
            onClick={toggleClickThrough}
            title="穿透点击（开启后按 Esc 关闭）"
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
          <button type="button" class="pinned-btn pinned-close" onClick={doClose} title="关闭">
            ×
          </button>
        </div>
      </div>
      <div
        class="pinned-body"
        ref={bodyRef}
        style={{
          transform: `scale(${scale()})`,
          "transform-origin": "top left",
          opacity: opacity(),
        }}
      >
        <Show when={data()}>
          <div
            class="ocr-body"
            classList={{ side: placeSide(data()) }}
          >
            <div
              class="ocr-shot"
              data-tauri-drag-region
              style={{
                width: `${data().width / dpr()}px`,
                height: `${data().height / dpr()}px`,
              }}
            >
              <img
                class="overlay-img-result"
                src={`data:image/png;base64,${data().b64}`}
                draggable={false}
                alt=""
              />
              <Show when={!showOriginal()}>
                <For each={translated()}>{(b) => <OcrOutline block={b} />}</For>
              </Show>
            </div>
            <Show when={!showOriginal()}>
              <div class="ocr-cards" style={{ "min-width": `${CARDS_MIN}px` }}>
                <For each={translated()}>
                  {(b) => <OcrBlock block={b} onSized={fitWindow} />}
                </For>
              </div>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

export default Pinned;
