import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import "./tokens.css";
import "./overlay.css";

// 全屏框选层：显示刚截下的整屏图片，用户拖出矩形，Esc 取消。
// 坐标统一用物理像素；显示时按 devicePixelRatio 缩回 CSS 像素，保证 1:1 对齐。
function Overlay() {
  const [shot, setShot] = createSignal(null);
  const [drag, setDrag] = createSignal(null);
  let canvasRef;

  const dpr = () => window.devicePixelRatio || 1;

  onMount(() => {
    invoke("get_screenshot")
      .then((data) => {
        setShot(data);
        const canvas = canvasRef;
        canvas.width = data.width;
        canvas.height = data.height;
        canvas.style.width = `${data.width / dpr()}px`;
        canvas.style.height = `${data.height / dpr()}px`;
        const bytes = Uint8ClampedArray.from(atob(data.b64), (c) => c.charCodeAt(0));
        const ctx = canvas.getContext("2d");
        ctx.putImageData(new ImageData(bytes, data.width, data.height), 0, 0);
      })
      .catch(() => invoke("overlay_cancel"));

    const onKey = (e) => {
      if (e.key === "Escape") invoke("overlay_cancel");
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  function phys(e) {
    const rect = canvasRef.getBoundingClientRect();
    return {
      x: (e.clientX - rect.left) * dpr(),
      y: (e.clientY - rect.top) * dpr(),
    };
  }

  function onDown(e) {
    const p = phys(e);
    setDrag({ x0: p.x, y0: p.y, x1: p.x, y1: p.y });
  }

  function onMove(e) {
    const d = drag();
    if (!d) return;
    const p = phys(e);
    setDrag({ x0: d.x0, y0: d.y0, x1: p.x, y1: p.y });
  }

  function onUp(e) {
    const d = drag();
    if (!d) return;
    const p = phys(e);
    const x = Math.round(Math.min(d.x0, p.x));
    const y = Math.round(Math.min(d.y0, p.y));
    const w = Math.round(Math.abs(p.x - d.x0));
    const h = Math.round(Math.abs(p.y - d.y0));
    setDrag(null);
    if (w < 4 || h < 4) {
      invoke("overlay_cancel");
      return;
    }
    invoke("overlay_select", { x, y, width: w, height: h });
  }

  function selStyle() {
    const d = drag();
    if (!d) return null;
    const x = Math.min(d.x0, d.x1);
    const y = Math.min(d.y0, d.y1);
    const w = Math.abs(d.x1 - d.x0);
    const h = Math.abs(d.y1 - d.y0);
    return {
      left: `${x / dpr()}px`,
      top: `${y / dpr()}px`,
      width: `${w / dpr()}px`,
      height: `${h / dpr()}px`,
    };
  }

  return (
    <div class="overlay-root" onMouseDown={onDown} onMouseMove={onMove} onMouseUp={onUp}>
      <canvas ref={canvasRef} class="overlay-canvas" />
      <Show when={selStyle()}>
        <div class="overlay-sel" style={selStyle()} />
      </Show>
      <div class="overlay-hint">拖出矩形框选要翻译的区域，Esc 取消</div>
    </div>
  );
}

export default Overlay;
