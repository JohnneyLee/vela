import { onMount } from "solid-js";

// 译文卡片：不盖原文，放在截图附近的列表里，字号固定可读。

export function OcrBlock(props) {
  const b = props.block;
  const onSized = props.onSized;
  let elRef;

  onMount(() => {
    requestAnimationFrame(() => {
      if (typeof onSized === "function") onSized();
    });
  });

  return (
    <div ref={elRef} class="ocr-card">
      <div class="ocr-card-dst">{b.translated}</div>
    </div>
  );
}

/** 原文位置的细线框，不遮挡文字。 */
export function OcrOutline(props) {
  const b = props.block;
  const dpr = () => window.devicePixelRatio || 1;
  return (
    <div
      class="ocr-outline"
      style={{
        left: `${b.x / dpr()}px`,
        top: `${b.y / dpr()}px`,
        width: `${Math.max(b.width / dpr(), 4)}px`,
        height: `${Math.max(b.height / dpr(), 4)}px`,
      }}
    />
  );
}
