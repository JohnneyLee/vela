import { createMemo, createSignal, onCleanup, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./tokens.css";
import "./App.css";

function normalizeWhitespace(text) {
  return text.trim().split(/\s+/).join(" ");
}

function tokenize(text) {
  const out = [];
  const re = /[A-Za-z0-9']+|[\u4e00-\u9fff]|[^\sA-Za-z0-9'\u4e00-\u9fff]+/g;
  let m;
  while ((m = re.exec(text)) !== null) out.push(m[0]);
  return out;
}

function diffWords(oldText, newText) {
  const oldTok = tokenize(oldText);
  const newTok = tokenize(newText);
  const m = oldTok.length;
  const n = newTok.length;
  if (m === 0 || n === 0 || m * n > 4_000_000) {
    const ops = [];
    if (oldText) ops.push({ type: "del", text: oldText });
    if (newText) ops.push({ type: "add", text: newText });
    return ops;
  }
  const dp = new Array(m + 1);
  for (let i = 0; i <= m; i++) dp[i] = new Uint16Array(n + 1);
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      dp[i][j] =
        oldTok[i] === newTok[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  const ops = [];
  let i = 0;
  let j = 0;
  while (i < m && j < n) {
    if (oldTok[i] === newTok[j]) {
      ops.push({ type: "same", text: oldTok[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push({ type: "del", text: oldTok[i] });
      i++;
    } else {
      ops.push({ type: "add", text: newTok[j] });
      j++;
    }
  }
  while (i < m) {
    ops.push({ type: "del", text: oldTok[i] });
    i++;
  }
  while (j < n) {
    ops.push({ type: "add", text: newTok[j] });
    j++;
  }
  return ops;
}

const MODES = [
  { id: "translate", label: "译" },
  { id: "explain", label: "释" },
  { id: "polish", label: "润" },
  { id: "speak", label: "读" },
];

const AI_KEY_FLAGS = {
  deepseek: "hasDeepseekKey",
  deepl: "hasDeeplKey",
  openai: "hasOpenaiKey",
  moonshot: "hasMoonshotKey",
  gemini: "hasGeminiKey",
};

function App() {
  const [source, setSource] = createSignal("");
  const [mtText, setMtText] = createSignal("");
  const [mtService, setMtService] = createSignal("");
  const [mtError, setMtError] = createSignal("");
  const [aiText, setAiText] = createSignal("");
  const [aiError, setAiError] = createSignal("");
  const [mtStatus, setMtStatus] = createSignal("idle");
  const [aiStatus, setAiStatus] = createSignal("idle");
  const [error, setError] = createSignal("");
  const [meta, setMeta] = createSignal("");
  const [mtSource, setMtSource] = createSignal("google");
  const [aiSource, setAiSource] = createSignal("free");
  const [hasDeeplKey, setHasDeeplKey] = createSignal(false);
  const [hasDeepseekKey, setHasDeepseekKey] = createSignal(false);
  const [hasOpenaiKey, setHasOpenaiKey] = createSignal(false);
  const [hasMoonshotKey, setHasMoonshotKey] = createSignal(false);
  const [hasGeminiKey, setHasGeminiKey] = createSignal(false);
  const [direction, setDirection] = createSignal("auto");
  const [style, setStyle] = createSignal("none");
  const [mtNote, setMtNote] = createSignal("");
  const [mode, setMode] = createSignal("translate");
  const [fromSelection, setFromSelection] = createSignal(false);
  const [editable, setEditable] = createSignal(false);
  const [actionMsg, setActionMsg] = createSignal("");
  const [speaking, setSpeaking] = createSignal(false);
  const [polishLevel, setPolishLevel] = createSignal("standard");
  const [diffExplanation, setDiffExplanation] = createSignal("");
  // UI state
  const [showSource, setShowSource] = createSignal(false);
  const [showOptions, setShowOptions] = createSignal(false);
  const [manualDraft, setManualDraft] = createSignal("");
  const [aiForSource, setAiForSource] = createSignal("");
  const [justReplaced, setJustReplaced] = createSignal(false);
  // Display mode: auto | ai-first | mt-first | compare
  const [displayMode, setDisplayMode] = createSignal("compare");
  // Which result is primary in translate mode ("ai" | "mt"); null in compare.
  const [primary, setPrimary] = createSignal("mt");
  // True while the AI is still generating after the 0.6s race fell back to MT.
  const [aiGenerating, setAiGenerating] = createSignal(false);

  const diffOps = createMemo(() => diffWords(source(), aiText()));
  const edited = {
    mtSource: false,
    aiSource: false,
    style: false,
  };
  let mtGeneration = 0;
  let aiGeneration = 0;
  let settingsLoaded = false;
  let saveChain = Promise.resolve();
  let resolveSettingsReady = () => {};
  const settingsReady = new Promise((resolve) => {
    resolveSettingsReady = resolve;
  });
  let aiTimer = 0;
  let heightCap = null; // user-dragged height cap in logical px; null = default
  let lastHeight = 0;
  const cardEl = () => document.getElementById("card");

  function currentSettings() {
    return {
      mtSource: mtSource(),
      aiSource: aiSource(),
      sourceHeight: 72,
      style: style(),
      displayMode: displayMode(),
    };
  }

  function clearMt() {
    setMtText("");
    setMtService("");
    setMtError("");
  }

  function clearAi() {
    setAiText("");
    setAiError("");
    setAiForSource("");
  }

  function clearResult() {
    mtGeneration += 1;
    aiGeneration += 1;
    setSource("");
    clearMt();
    clearAi();
    setError("");
    setMeta("");
    setMtStatus("idle");
    setAiStatus("idle");
    setFromSelection(false);
    setActionMsg("");
    setSpeaking(false);
    setDiffExplanation("");
    setJustReplaced(false);
    setAiGenerating(false);
  }

  function saveSettings() {
    if (!settingsLoaded) return Promise.resolve();
    const settings = currentSettings();
    const run = saveChain.catch(() => {}).then(() => invoke("save_settings", { settings }));
    saveChain = run.catch(() => {});
    return run;
  }

  function aiAvailable() {
    const s = aiSource();
    if (s === "free" || s === "ollama") return true;
    const flag = AI_KEY_FLAGS[s];
    if (!flag) return false;
    return (
      (s === "deepseek" && hasDeepseekKey()) ||
      (s === "deepl" && hasDeeplKey()) ||
      (s === "openai" && hasOpenaiKey()) ||
      (s === "moonshot" && hasMoonshotKey()) ||
      (s === "gemini" && hasGeminiKey())
    );
  }

  function resolveDisplayMode() {
    const dm = displayMode();
    if (dm === "ai-first") return "ai-first";
    if (dm === "mt-first") return "mt-first";
    if (dm === "compare") return "compare";
    return aiAvailable() ? "ai-first" : "mt-first";
  }

  async function requestSide(side, text, ticket) {
    const isMt = side === "mt";
    const trimmed = String(text ?? "").trim();
    if (isMt) {
      clearMt();
      setMtNote("");
    } else clearAi();
    if (!trimmed) {
      if (isMt) setMtStatus("idle");
      else setAiStatus("idle");
      return;
    }
    if (isMt) setMtStatus("loading");
    else setAiStatus("loading");
    try {
      await settingsReady;
      await saveChain.catch(() => {});
      if (isMt ? ticket !== mtGeneration : ticket !== aiGeneration) return;
      const res = await invoke("translate_text", {
        text: trimmed,
        mtSource: mtSource(),
        aiSource: aiSource(),
        side,
        style: style(),
        direction: direction(),
        mode: mode(),
        polishLevel: polishLevel(),
        ticket,
      });
      if (isMt ? ticket !== mtGeneration : ticket !== aiGeneration) return;
      if (res?.cancelled) return;
      if (res?.directionLabel) setMeta(res.directionLabel);
      if (isMt) {
        setMtText(res?.mtText ?? "");
        setMtService(res?.mtService ?? "");
        setMtError(res?.mtError ?? "");
        setMtStatus(res?.mtError ? "error" : "done");
      } else {
        setAiText(res?.aiText ?? "");
        setAiError(res?.aiError ?? "");
        setAiStatus(res?.aiError ? "error" : "done");
        setAiForSource(trimmed);
        // AI finished. If we fell back to MT during the race, promote AI now;
        // if AI failed and MT is available, keep MT as primary.
        if (mode() === "translate") {
          if (res?.aiText) {
            if (aiGenerating() || resolveDisplayMode() === "ai-first") {
              setAiGenerating(false);
              if (resolveDisplayMode() === "ai-first") setPrimary("ai");
            }
          } else if (res?.aiError) {
            setAiGenerating(false);
            if (primary() === "ai" && mtText()) setPrimary("mt");
          }
        }
      }
    } catch (err) {
      if (isMt ? ticket !== mtGeneration : ticket !== aiGeneration) return;
      if (isMt) {
        setMtError(String(err));
        setMtStatus("error");
      } else {
        setAiError(String(err));
        setAiStatus("error");
        setAiGenerating(false);
        if (mode() === "translate" && primary() === "ai" && mtText()) setPrimary("mt");
      }
    }
  }

  function translateSide(side, text) {
    const ticket = side === "mt" ? ++mtGeneration : ++aiGeneration;
    requestSide(side, text, ticket);
    return ticket;
  }

  function runTranslate(text) {
    mtGeneration += 1;
    aiGeneration += 1;
    const mtTicket = mtGeneration;
    const aiTicket = aiGeneration;
    clearMt();
    clearAi();
    setMtStatus("loading");
    setAiStatus("loading");
    setAiGenerating(false);
    setError("");
    setMeta("");
    setShowOptions(false);

    const dm = resolveDisplayMode();
    if (dm === "compare") {
      setPrimary("mt");
    } else {
      setPrimary(dm === "ai-first" ? "ai" : "mt");
    }

    requestSide("mt", text, mtTicket);
    requestSide("ai", text, aiTicket);

    // 0.6s race: if the AI is primary but has not started streaming yet, fall
    // back to showing MT (with an "AI 生成中" note) until the AI arrives.
    if (dm === "ai-first") {
      window.clearTimeout(aiTimer);
      aiTimer = window.setTimeout(() => {
        if (aiGeneration === aiTicket && aiStatus() === "loading" && !aiText()) {
          setAiGenerating(true);
          if (primary() === "ai") setPrimary("mt");
        }
      }, 600);
    }
  }

  function runAction(action) {
    const text = source().trim();
    if (!text) return;
    if (action === "speak") {
      const result = resultToSpeak();
      setMode("speak");
      toggleSpeak(result || text);
      return;
    }
    setMode(action);
    setDiffExplanation("");
    setJustReplaced(false);
    if (action === "translate") {
      runTranslate(text);
    } else {
      // Explain/polish use the AI column directly.
      setAiForSource("");
      setPrimary("mt");
      setAiGenerating(false);
      clearMt();
      translateSide("ai", text);
    }
  }

  function swapResults() {
    setPrimary((p) => (p === "ai" ? "mt" : "ai"));
  }

  function replaceText() {
    const text = (mode() === "polish" || mode() === "explain"
      ? aiText()
      : primary() === "ai" ? aiText() : mtText()
    ).trim();
    if (!text) return;
    setActionMsg("");
    invoke("replace_text", { text, original: source() })
      .then(() => {
        setActionMsg("已替换。可点「还原」撤销。");
        setJustReplaced(true);
      })
      .catch((err) => setActionMsg(String(err)));
  }

  function copySpecific(text) {
    const t = String(text ?? "").trim();
    if (!t) return;
    invoke("copy_text", { text: t })
      .then(() => setActionMsg("已复制到剪贴板。"))
      .catch((err) => setActionMsg(String(err)));
  }

  function replaceSpecific(text) {
    const t = String(text ?? "").trim();
    if (!t) return;
    setActionMsg("");
    invoke("replace_text", { text: t, original: source() })
      .then(() => {
        setActionMsg("已替换。可点「还原」撤销。");
        setJustReplaced(true);
      })
      .catch((err) => setActionMsg(String(err)));
  }

  function copyText() {
    const text = (mode() === "polish" || mode() === "explain"
      ? aiText()
      : primary() === "ai" ? aiText() : mtText()
    ).trim();
    if (!text) return;
    invoke("copy_text", { text })
      .then(() => setActionMsg("已复制到剪贴板。"))
      .catch((err) => setActionMsg(String(err)));
  }

  function undoText() {
    setActionMsg("");
    invoke("undo_text")
      .then((original) => {
        setActionMsg(original ? "已还原原文。" : "已发送撤销。");
      })
      .catch((err) => setActionMsg(String(err)));
  }

  function explainDiff() {
    const original = source().trim();
    const polished = aiText().trim();
    if (!original || !polished) return;
    setDiffExplanation("");
    invoke("explain_diff", { original, polished })
      .then((text) => setDiffExplanation(String(text ?? "")))
      .catch((err) => setActionMsg(String(err)));
  }

  function speakSource() {
    const text = source().trim();
    if (!text) return;
    toggleSpeak(text);
  }

  function toggleSpeak(text) {
    if (speaking()) {
      setSpeaking(false);
      invoke("stop_speak");
      return;
    }
    setSpeaking(true);
    setActionMsg("");
    invoke("speak", { text })
      .catch((err) => setActionMsg(String(err)))
      .finally(() => setSpeaking(false));
  }

  // 朗读主按钮读「结果」：润色/解释读 AI 结果，翻译读当前主结果；
  // 原文由原文行的小喇叭朗读。
  function resultToSpeak() {
    if (mode() === "explain" || mode() === "polish") return aiText();
    if (mode() === "translate") {
      const p = primary();
      if (p === "ai") return aiText() || mtText();
      if (p === "mt") return mtText() || aiText();
      return aiText() || mtText();
    }
    return "";
  }

  function onSetting(flag, apply) {
    apply();
    const rerun = () => {
      if (!source().trim()) return;
      if (flag === "direction") runAction(mode());
      else if (flag === "style" || flag === "aiSource") translateSide("ai", source());
      else if (flag === "mtSource") translateSide("mt", source());
    };
    // 方向只对这一次有效，立即重跑但不写入设置。
    if (flag === "direction") {
      rerun();
      return;
    }
    edited[flag] = true;
    saveSettings()
      .then(rerun)
      .catch(() => {});
  }

  function openSettings() {
    invoke("open_settings");
  }

  function dismissWindow() {
    invoke("dismiss");
  }

  function maxByWorkArea() {
    return Math.floor((window.screen.availHeight || 800) * 0.7);
  }

  function syncHeight() {
    const el = cardEl();
    if (!el) return;
    const maxH = heightCap ?? maxByWorkArea();
    el.style.maxHeight = `${Math.max(maxH, 160)}px`;
    const h = Math.ceil(el.getBoundingClientRect().height);
    if (h !== lastHeight) {
      lastHeight = h;
      invoke("panel_resize", { height: h }).catch(() => {});
    }
  }

  function startResize(direction) {
    const win = getCurrentWindow();
    win.startResizeDragging(direction)
      .then(async () => {
        if (direction !== "South") return;
        // User dragged the bottom edge. If they dragged taller than the
        // content, remember that as the height cap and snap back to content.
        try {
          const scale = await win.scaleFactor();
          const size = await win.innerSize();
          const winH = size.height / scale;
          const el = cardEl();
          const contentH = el ? Math.ceil(el.getBoundingClientRect().height) : winH;
          if (winH > contentH + 4) {
            heightCap = winH;
            lastHeight = contentH;
            invoke("panel_resize", { height: contentH }).catch(() => {});
          }
        } catch (_) {
          /* ignore */
        }
      })
      .catch(() => {});
  }

  function manualTranslate() {
    const text = manualDraft().trim();
    if (!text) return;
    setSource(text);
    setFromSelection(false);
    setEditable(false);
    runAction("translate");
  }

  onMount(() => {
    const unsubs = [];

    invoke("get_settings")
      .then((settings) => {
        if (!settings) return;
        if (!edited.mtSource) setMtSource(settings.mtSource || "google");
        if (!edited.aiSource) setAiSource(settings.aiSource || "free");
        setHasDeeplKey(Boolean(settings.hasDeeplKey));
        setHasDeepseekKey(Boolean(settings.hasDeepseekKey));
        setHasOpenaiKey(Boolean(settings.hasOpenaiKey));
        setHasMoonshotKey(Boolean(settings.hasMoonshotKey));
        setHasGeminiKey(Boolean(settings.hasGeminiKey));
        if (!edited.style) setStyle(settings.style || "none");
        setDisplayMode(settings.displayMode || "compare");
      })
      .catch(() => {})
      .finally(() => {
        settingsLoaded = true;
        resolveSettingsReady();
      });

    listen("selection-captured", (event) => {
      const raw = String(event.payload?.text ?? "");
      const text = normalizeWhitespace(raw);
      const action = String(event.payload?.action ?? "translate");
      // 手动方向只对这一次选择有效；新选择回到「自动」。
      setDirection("auto");
      setSource(text);
      setFromSelection(true);
      setEditable(Boolean(event.payload?.editable));
      setDiffExplanation("");
      if (action && action !== "translate") {
        runAction(action);
      } else {
        runAction("translate");
      }
    }).then((unsub) => unsubs.push(unsub));

    listen("selection-error", (event) => {
      mtGeneration += 1;
      aiGeneration += 1;
      setSource("");
      clearMt();
      clearAi();
      setMeta("");
      setMtStatus("idle");
      setAiStatus("idle");
      setError(typeof event.payload === "string" ? event.payload : "取词失败");
    }).then((unsub) => unsubs.push(unsub));

    listen("result-dismissed", () => {
      clearResult();
    }).then((unsub) => unsubs.push(unsub));

    listen("ai-chunk", (event) => {
      if (event.payload?.ticket !== aiGeneration) return;
      setAiText(event.payload?.text ?? "");
      // First AI token arrived: if we fell back to MT during the race, promote.
      if (mode() === "translate" && aiGenerating()) {
        setAiGenerating(false);
        if (resolveDisplayMode() === "ai-first") setPrimary("ai");
      }
    }).then((unsub) => unsubs.push(unsub));

    listen("mt-status", (event) => {
      if (event.payload?.ticket !== mtGeneration) return;
      setMtNote(event.payload?.text ?? "");
    }).then((unsub) => unsubs.push(unsub));

    listen("hotkey-error", (event) => {
      setActionMsg(typeof event.payload === "string" ? event.payload : "快捷键 Ctrl+Alt+T 已被占用。");
    }).then((unsub) => unsubs.push(unsub));

    const onKey = (event) => {
      if (event.key === "Escape") {
        if (showOptions()) setShowOptions(false);
        else invoke("dismiss");
      }
    };
    window.addEventListener("keydown", onKey);

    // Keep the panel height in sync with the card content (throttled). The
    // card's max-height is a fixed pixel value (work area × 0.7, or the
    // user-dragged cap), never tied to the window height, so this converges.
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(syncHeight);
    });
    const el = cardEl();
    if (el) ro.observe(el);
    syncHeight();

    onCleanup(() => {
      window.clearTimeout(aiTimer);
      window.removeEventListener("keydown", onKey);
      ro.disconnect();
      unsubs.forEach((unsub) => unsub());
    });
  });

  function resultText() {
    if (mode() === "explain" || mode() === "polish") return aiText();
    return primary() === "ai" ? aiText() : mtText();
  }

  function hasResult() {
    if (mode() === "explain" || mode() === "polish") return Boolean(aiText());
    return Boolean(mtText() || aiText());
  }

  function mtEngineLabel() {
    switch (mtService()) {
      case "mymemory":
        return "MyMemory";
      case "offline":
        return "离线";
      case "google":
        return "Google";
      default:
        return "机器翻译";
    }
  }

  function aiEngineLabel() {
    switch (aiSource()) {
      case "deepseek":
        return "DeepSeek";
      case "deepl":
        return "DeepL";
      case "openai":
        return "OpenAI";
      case "moonshot":
        return "Moonshot";
      case "gemini":
        return "Gemini";
      case "ollama":
        return "Ollama";
      default:
        return "免费";
    }
  }

  function styleLabel() {
    switch (style()) {
      case "academic":
        return "学术";
      case "colloquial":
        return "口语";
      case "formal":
        return "正式";
      case "idiomatic":
        return "地道";
      case "literal":
        return "直译";
      default:
        return "自然";
    }
  }

  function modeLabel() {
    switch (mode()) {
      case "explain":
        return "解释";
      case "polish":
        return "润色";
      case "speak":
        return "朗读";
      default:
        return "翻译";
    }
  }

  function headLabel() {
    if (mode() === "translate") return meta() || "自动";
    return modeLabel();
  }

  const aiLabel = () => `AI · ${aiEngineLabel()} · ${styleLabel()}`;
  const mtLabel = () => `机器翻译 · ${mtEngineLabel()}`;

  return (
    <div class="card" id="card">
      {/* 无边框窗口的缩放手柄 */}
      <div class="resize-se" onMouseDown={(e) => { e.preventDefault(); startResize("SouthEast"); }} />
      <div class="resize-s" onMouseDown={(e) => { e.preventDefault(); startResize("South"); }} />
      <div class="resize-e" onMouseDown={(e) => { e.preventDefault(); startResize("East"); }} />

      {/* 顶部：方向（左） + 居中标签 + 设置/关闭（右）。
          整个头部都可拖动窗口（无边框窗口的移动热区）；按钮自动排除。 */}
      <header class="card-head" data-tauri-drag-region>
        <div class="head-left" title="语言方向">
          {headLabel()}
        </div>
        <div class="mode-tabs">
          {MODES.map((m) => (
            <button
              type="button"
              class="mode-tab"
              classList={{ active: mode() === m.id }}
              aria-label={modeLabel()}
              onClick={() => {
                if (source().trim()) runAction(m.id);
              }}
            >
              {m.label}
            </button>
          ))}
        </div>
        <div class="head-right">
          <button type="button" class="icon-btn" onClick={openSettings} aria-label="设置" title="设置">
            ⚙
          </button>
          <button type="button" class="icon-btn" onClick={dismissWindow} aria-label="关闭" title="关闭">
            ×
          </button>
        </div>
      </header>

      {/* 错误横幅 */}
      <Show when={actionMsg() || error()}>
        <div class="banner">{actionMsg() || error()}</div>
      </Show>

      {/* 手动输入框（仅手动打开时） */}
      <Show when={!fromSelection() && !source()}>
        <div class="manual">
          <textarea
            class="manual-input"
            placeholder="输入文字，回车翻译"
            value={manualDraft()}
            onInput={(e) => setManualDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                manualTranslate();
              }
            }}
          />
        </div>
      </Show>

      {/* 原文行：折叠两行 + 朗读 */}
      <Show when={source()}>
        <div class="source-row" onClick={() => setShowSource(!showSource())}>
          <span class="source-text" classList={{ collapsed: !showSource() }}>
            {source()}
          </span>
          <span class="source-expand">{showSource() ? "▾" : "▸"}</span>
          <button
            type="button"
            class="mini-speak"
            classList={{ active: speaking() }}
            onClick={(e) => {
              e.stopPropagation();
              speakSource();
            }}
            aria-label={speaking() ? "停止朗读" : "朗读原文"}
            title={speaking() ? "停止朗读" : "朗读原文"}
          >
            {speaking() ? "⏹" : "🔊"}
          </button>
        </div>
      </Show>

      {/* 结果区 */}
      <div class="result-area">
        {/* 翻译模式：主次结果 */}
        <Show when={mode() === "translate"}>
          <Show when={resolveDisplayMode() === "compare"}>
            <div class="result-block">
              <div class="result-block-head">
                <span class="result-label">{mtLabel()}</span>
                <span class="block-actions">
                  <Show when={mtText()}>
                    <button type="button" class="block-btn" onClick={() => copySpecific(mtText())}>复制</button>
                    <Show when={fromSelection() && editable()}>
                      <button type="button" class="block-btn" onClick={() => replaceSpecific(mtText())}>替换</button>
                    </Show>
                  </Show>
                </span>
              </div>
              <Show when={mtStatus() === "loading" && !mtText()}>
                <div class="skeleton"><div class="sk" /><div class="sk" /></div>
              </Show>
              <Show when={mtText()}>
                <div class="result-text">{mtText()}</div>
              </Show>
              <Show when={mtError()}>
                <div class="ai-error-note">{mtError()}</div>
              </Show>
            </div>
            <div class="result-block">
              <div class="result-block-head">
                <span class="result-label">{aiLabel()}</span>
                <span class="block-actions">
                  <Show when={aiText()}>
                    <button type="button" class="block-btn" onClick={() => copySpecific(aiText())}>复制</button>
                    <Show when={fromSelection() && editable()}>
                      <button type="button" class="block-btn" onClick={() => replaceSpecific(aiText())}>替换</button>
                    </Show>
                  </Show>
                </span>
              </div>
              <Show when={aiStatus() === "loading" && !aiText()}>
                <div class="skeleton"><div class="sk" /></div>
              </Show>
              <Show when={aiText()}>
                <div class="result-text">{aiText()}</div>
              </Show>
              <Show when={aiError()}>
                <div class="ai-error-note">{aiError()}</div>
              </Show>
            </div>
          </Show>

          <Show when={resolveDisplayMode() !== "compare"}>
            {/* 主结果 */}
            <Show when={primary() === "ai"}>
              <div class="result-label">{aiLabel()}</div>
              <Show when={aiStatus() === "loading" && !aiText()}>
                <div class="skeleton"><div class="sk" /><div class="sk" /></div>
              </Show>
              <Show when={aiText()}>
                <div class="result-text">{aiText()}</div>
              </Show>
            </Show>
            <Show when={primary() === "mt"}>
              <div class="result-label">{mtLabel()}</div>
              <Show when={mtStatus() === "loading" && !mtText()}>
                <div class="skeleton"><div class="sk" /><div class="sk" /></div>
              </Show>
              <Show when={mtText()}>
                <div class="result-text">{mtText()}</div>
              </Show>
              <Show when={aiGenerating()}>
                <div class="ai-generating">AI 生成中…</div>
              </Show>
              <Show when={aiError()}>
                <div class="ai-error-note">{aiError()}</div>
              </Show>
            </Show>

            {/* 次结果：细灰行，点击互换 */}
            <Show when={primary() === "ai" && mtText()}>
              <button type="button" class="secondary-row" onClick={swapResults}>
                <span class="secondary-label">{mtLabel()}</span>
                <span class="secondary-snippet">{mtText()}</span>
                <span class="secondary-arrow">▸</span>
              </button>
            </Show>
            <Show when={primary() === "mt" && aiText()}>
              <button type="button" class="secondary-row" onClick={swapResults}>
                <span class="secondary-label">{aiLabel()}</span>
                <span class="secondary-snippet">{aiText()}</span>
                <span class="secondary-arrow">▸</span>
              </button>
            </Show>
          </Show>
        </Show>

        {/* 解释/润色：单一 AI 结果 */}
        <Show when={mode() === "explain" || mode() === "polish"}>
          <div class="result-label">AI · {aiEngineLabel()}</div>
          <Show when={aiStatus() === "loading" && !aiText()}>
            <div class="skeleton"><div class="sk" /><div class="sk" /></div>
          </Show>
          <Show when={mode() === "polish" && aiText()}>
            <div class="diff-view">
              {diffOps().map((op) => {
                if (op.type === "del") return <del class="diff-del">{op.text} </del>;
                if (op.type === "add") return <ins class="diff-add">{op.text} </ins>;
                return <span>{op.text} </span>;
              })}
            </div>
          </Show>
          <Show when={mode() !== "polish" && aiText()}>
            <div class="result-text">{aiText()}</div>
          </Show>
          <Show when={aiError()}>
            <div class="ai-error-note">{aiError()}</div>
          </Show>
          <Show when={diffExplanation()}>
            <p class="diff-note">{diffExplanation()}</p>
          </Show>
        </Show>
      </div>

      {/* 操作按钮 */}
      <Show when={hasResult()}>
        <div class="card-actions">
          {/* 对照模式下复制/替换在每个结果块上，这里只保留还原 */}
          <Show when={!(mode() === "translate" && resolveDisplayMode() === "compare")}>
            <button type="button" class="act" onClick={copyText}>复制</button>
            <Show when={fromSelection() && editable() && mode() !== "explain" && mode() !== "speak"}>
              <button type="button" class="act" onClick={replaceText}>替换</button>
            </Show>
          </Show>
          <Show when={fromSelection() && justReplaced()}>
            <button type="button" class="act" onClick={undoText}>还原</button>
          </Show>
          <Show when={mode() === "polish"}>
            <button type="button" class="act" onClick={explainDiff}>为什么改</button>
          </Show>
        </div>
      </Show>

      {/* 底部：只剩 ···（双击空白清高度上限） */}
      <footer class="card-foot" onDblClick={() => { heightCap = null; lastHeight = 0; syncHeight(); }}>
        <button type="button" class="more" onClick={() => setShowOptions(!showOptions())} aria-label="选项">
          ···
        </button>
      </footer>

      {/* 选项浮层 */}
      <Show when={showOptions()}>
        <div class="options">
          <Show when={mode() === "translate"}>
            <label class="opt-row">
              <span>方向</span>
              <select
                value={direction()}
                onChange={(e) => onSetting("direction", () => setDirection(e.currentTarget.value))}
              >
                <option value="auto">自动</option>
                <option value="zh-en">中 → 英</option>
                <option value="en-zh">英 → 中</option>
              </select>
            </label>
            <label class="opt-row">
              <span>机器翻译</span>
              <select
                value={mtSource()}
                onChange={(e) => onSetting("mtSource", () => setMtSource(e.currentTarget.value))}
              >
                <option value="google">Google</option>
                <option value="mymemory">MyMemory</option>
                <option value="offline">离线</option>
              </select>
            </label>
            <label class="opt-row">
              <span>风格</span>
              <select value={style()} onChange={(e) => onSetting("style", () => setStyle(e.currentTarget.value))}>
                <option value="none">自然</option>
                <option value="academic">学术</option>
                <option value="colloquial">口语</option>
                <option value="formal">正式</option>
                <option value="idiomatic">地道</option>
                <option value="literal">直译</option>
              </select>
            </label>
          </Show>
          <Show when={mode() === "polish"}>
            <label class="opt-row">
              <span>力度</span>
              <select
                value={polishLevel()}
                onChange={(e) => {
                  setPolishLevel(e.currentTarget.value);
                  if (source().trim()) translateSide("ai", source());
                }}
              >
                <option value="light">只改错误</option>
                <option value="standard">标准</option>
                <option value="heavy">大幅重写</option>
              </select>
            </label>
          </Show>
          <label class="opt-row">
            <span>AI 引擎</span>
            <select value={aiSource()} onChange={(e) => onSetting("aiSource", () => setAiSource(e.currentTarget.value))}>
              <option value="free">免费</option>
              <option value="deepseek">DeepSeek</option>
              <option value="deepl">DeepL</option>
              <option value="openai">OpenAI</option>
              <option value="moonshot">Moonshot</option>
              <option value="gemini">Gemini</option>
              <option value="ollama">Ollama</option>
            </select>
          </label>
        </div>
      </Show>
    </div>
  );
}

export default App;
