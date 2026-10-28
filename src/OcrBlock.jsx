import { onMount } from "solid-js";

// 一个译文块：用边框采样出的背景色擦除原文，再写译文。字号从行高估计，
// 放不下就逐步缩小到 9px。Overlay 结果层和 Pinned 钉住窗口共用。
export function OcrBlock(props) {
  const b = props.block;
  let elRef;

  onMount(() => {
    const el = elRef;
    const dpr = window.devicePixelRatio || 1;
    const hCss = b.height / dpr;
    let size = Math.max(9, Math.min(hCss * 0.8, 32));
    el.style.fontSize = `${size}px`;
    // 逐步缩小，直到译文放进原文矩形，或到达最小 9px。
    while (size > 9 && el.scrollHeight > el.clientHeight + 1) {
      size -= 1;
      el.style.fontSize = `${size}px`;
    }
  });

  const dpr = () => window.devicePixelRatio || 1;

  return (
    <div
      ref={elRef}
      class="ocr-block"
      style={{
        left: `${b.x / dpr()}px`,
        top: `${b.y / dpr()}px`,
        width: `${b.width / dpr()}px`,
        height: `${b.height / dpr()}px`,
        "background-color": b.solidBg ? b.bgColor : `${b.bgColor}d9`,
        color: b.textColor,
      }}
    >
      {b.translated}
    </div>
  );
}
