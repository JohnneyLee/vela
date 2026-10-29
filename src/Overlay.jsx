import { createSignal, onCleanup, onMount, Show, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { OcrBlock, OcrOutline } from "./OcrBlock";
import "./tokens.css";
import "./overlay.css";

// 框选 → OCR/翻译。原文截图保留；译文放在右侧（更合适时），不盖原文。

const CHROME_CSS = 36;
const CARDS_MIN = 220;

function Overlay() {
  const [imgUrl, setImgUrl] = createSignal("");
  const [drag, setDrag] = createSignal(null);
  const [result, setResult] = createSignal(null);
  const [busy, setBusy] = createSignal(false);
  let imgRef;
  let bodyRef;

  const dpr = () => window.devicePixelRatio || 1;

  function toDataUrl(b64) {
    return `data:image/png;base64,${b64}`;
  }

  /** 截图偏高/方正时译文放右侧；特别扁宽时放下方。 */
  function placeSide(data) {
    if (!data) return true;
    return data.width / Math.max(1, data.height) < 2.2;
  }

  function loadScreenshot() {
    setResult(null);
    setBusy(false);
    setDrag(null);
    invoke("get_screenshot")
      .then((data) => setImgUrl(toDataUrl(data.b64)))
      .catch((err) => {
        console.error("get_screenshot failed", err);
        invoke("overlay_cancel");
      });
  }

  function fitWindow() {
    const data = result();
    if (!data || !bodyRef) return;
    const side = placeSide(data);
    const shotW = data.width / dpr();
    const shotH = data.height / dpr();
    const cards = bodyRef.querySelector(".ocr-cards");
    const cardsW = cards ? Math.max(cards.scrollWidth, CARDS_MIN) : CARDS_MIN;
    const cardsH = cards ? cards.scrollHeight : 0;
    const w = side ? shotW + cardsW : Math.max(shotW, cardsW);
    const h = CHROME_CSS + (side ? Math.max(shotH, cardsH) : shotH + cardsH);
    getCurrentWindow()
      .setSize(new PhysicalSize(Math.ceil(w * dpr()), Math.ceil(h * dpr())))
      .catch(() => {});
  }

  onMount(() => {
    const unsubs = [];
    loadScreenshot();
    listen("screenshot-ready", () => loadScreenshot()).then((u) => unsubs.push(u));
    listen("ocr-result-ready", () => {
      setBusy(false);
      invoke("get_ocr_result")
        .then((data) => {
          setResult(data);
          setImgUrl(toDataUrl(data.b64));
          requestAnimationFrame(() => fitWindow());
        })
        .catch(() => invoke("overlay_cancel"));
    }).then((u) => unsubs.push(u));

    const onKey = (e) => {
      if (e.key === "Escape") invoke("overlay_cancel");
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      unsubs.forEach((u) => u());
    });
  });

  function onDown(e) {
    if (result() || busy()) return;
    if (e.button !== 0) {
      invoke("overlay_cancel");
      return;
    }
    setDrag({ x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY });
  }

  function onMove(e) {
    const d = drag();
    if (!d) return;
    setDrag({ x0: d.x0, y0: d.y0, x1: e.clientX, y1: e.clientY });
  }

  function onUp(e) {
    const d = drag();
    if (!d) return;
    setDrag(null);
    const img = imgRef;
    if (!img || !img.naturalWidth) {
      invoke("overlay_cancel");
      return;
    }
    const dpi = img.naturalWidth / Math.max(1, window.innerWidth);
    const left = Math.floor(Math.min(d.x0, e.clientX) * dpi);
    const top = Math.floor(Math.min(d.y0, e.clientY) * dpi);
    const right = Math.floor(Math.max(d.x0, e.clientX) * dpi);
    const bottom = Math.floor(Math.max(d.y0, e.clientY) * dpi);
    const width = right - left;
    const height = bottom - top;
    if (width < 4 || height < 4) {
      invoke("overlay_cancel");
      return;
    }
    setBusy(true);
    invoke("overlay_select", { x: left, y: top, width, height }).catch((err) => {
      console.error("overlay_select failed", err);
      setBusy(false);
      invoke("overlay_cancel");
    });
  }

  function selStyle() {
    const d = drag();
    if (!d) return null;
    return {
      left: `${Math.min(d.x0, d.x1)}px`,
      top: `${Math.min(d.y0, d.y1)}px`,
      width: `${Math.abs(d.x1 - d.x0)}px`,
      height: `${Math.abs(d.y1 - d.y0)}px`,
    };
  }

  const translated = () => (result()?.blocks || []).filter((b) => b.translated);

  return (
    <div
      class="overlay-root"
      classList={{ result: !!result(), busy: busy() }}
      onMouseDown={onDown}
      onMouseMove={onMove}
      onMouseUp={onUp}
    >
      <Show when={!result() && imgUrl()}>
        <img ref={imgRef} class="overlay-img" src={imgUrl()} draggable={false} alt="" />
      </Show>
      <Show when={!result() && !busy() && selStyle()}>
        <div class="overlay-sel" style={selStyle()} />
      </Show>
      <Show when={!result() && !busy()}>
        <div class="overlay-hint">拖出矩形框选要翻译的区域，Esc 取消</div>
        <button
          type="button"
          class="ocr-tool-btn ocr-close"
          onClick={() => invoke("overlay_cancel")}
        >×</button>
      </Show>
      <Show when={busy()}>
        <div class="overlay-hint">正在识别并翻译…</div>
      </Show>

      <Show when={result()}>
        <div class="ocr-chrome">
          <span class="ocr-engine-badge">{result().engineLabel || "机器翻译"}</span>
          <div class="ocr-chrome-actions">
            <button type="button" class="ocr-tool-btn" onClick={() => invoke("pin_result").catch(() => {})}>
              📌 钉住
            </button>
            <button type="button" class="ocr-tool-btn" onClick={() => invoke("overlay_cancel")}>×</button>
          </div>
        </div>
        <div
          class="ocr-body"
          classList={{ side: placeSide(result()) }}
          ref={bodyRef}
        >
          <div
            class="ocr-shot"
            style={{
              width: `${result().width / dpr()}px`,
              height: `${result().height / dpr()}px`,
            }}
          >
            <img class="overlay-img-result" src={imgUrl()} draggable={false} alt="" />
            <For each={translated()}>{(b) => <OcrOutline block={b} />}</For>
          </div>
          <div class="ocr-cards" style={{ "min-width": `${CARDS_MIN}px` }}>
            <For each={translated()}>
              {(b) => <OcrBlock block={b} onSized={fitWindow} />}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}

export default Overlay;
