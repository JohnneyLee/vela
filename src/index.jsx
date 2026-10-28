/* @refresh reload */
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Bar from "./Bar";
import Panel from "./App";
import Settings from "./Settings";
import Overlay from "./Overlay";
import Pinned from "./Pinned";
import { applyTheme } from "./theme";

// 三个窗口共享一个入口：启动即读取主题并应用，随后监听主题变化广播。
invoke("get_settings")
  .then((s) => applyTheme(s?.theme))
  .catch(() => {});
listen("theme-changed", (event) => applyTheme(event.payload)).catch(() => {});

// Three windows share one bundle; the route is picked from the URL hash.
function route() {
  const hash = window.location.hash;
  if (hash.startsWith("#/settings")) return "settings";
  if (hash.startsWith("#/bar")) return "bar";
  if (hash.startsWith("#/overlay")) return "overlay";
  if (hash.startsWith("#/pinned")) return "pinned";
  return "panel";
}

const which = route();
const root = document.getElementById("root");
if (which === "bar") render(() => <Bar />, root);
else if (which === "settings") render(() => <Settings />, root);
else if (which === "overlay") render(() => <Overlay />, root);
else if (which === "pinned") render(() => <Pinned />, root);
else render(() => <Panel />, root);
