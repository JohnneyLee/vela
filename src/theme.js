// 统一应用主题到 <html data-theme>。
// - "light" / "dark"：强制，覆盖系统偏好；
// - 其他（含 "system"）：移除属性，交给 prefers-color-scheme。
export function applyTheme(theme) {
  const root = document.documentElement;
  const explicit = ["light", "dark", "green", "sepia"];
  if (explicit.includes(theme)) {
    root.dataset.theme = theme;
  } else {
    delete root.dataset.theme;
  }
}
