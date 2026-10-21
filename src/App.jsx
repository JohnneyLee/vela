import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

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
  const [deeplKey, setDeeplKey] = createSignal("");
  const [deepseekKey, setDeepseekKey] = createSignal("");
  const [hasDeeplKey, setHasDeeplKey] = createSignal(false);
  const [hasDeepseekKey, setHasDeepseekKey] = createSignal(false);
  const [masterPassword, setMasterPassword] = createSignal("");
  const [sourceHeight, setSourceHeight] = createSignal(72);
  const [direction, setDirection] = createSignal("auto");
  const [style, setStyle] = createSignal("none");
  const [mtNote, setMtNote] = createSignal("");
  const [notice, setNotice] = createSignal("");
  const edited = {
    mtSource: false,
    aiSource: false,
    deeplKey: false,
    deepseekKey: false,
    sourceHeight: false,
    direction: false,
    style: false,
  };
  let mtGeneration = 0;
  let aiGeneration = 0;
  let committedDeepseek = "";
  let committedDeepl = "";
  let settingsLoaded = false;
  let saveChain = Promise.resolve();
  let resolveSettingsReady = () => {};
  const settingsReady = new Promise((resolve) => {
    resolveSettingsReady = resolve;
  });

  function currentSettings() {
    return {
      mtSource: mtSource(),
      aiSource: aiSource(),
      sourceHeight: sourceHeight(),
      direction: direction(),
      style: style(),
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
  }

  function saveSettings() {
    if (!settingsLoaded) return Promise.resolve();
    const settings = currentSettings();
    const run = saveChain.catch(() => {}).then(() => invoke("save_settings", { settings }));
    saveChain = run.catch(() => {});
    return run;
  }

  async function translateSide(side, text) {
    const isMt = side === "mt";
    const ticket = isMt ? ++mtGeneration : ++aiGeneration;
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
      const needsPassword =
        !isMt && (aiSource() === "deepseek" || aiSource() === "deepl");
      if (needsPassword && !masterPassword().trim()) {
        setAiError("请输入主密码。主密码不会保存。");
        setAiStatus("error");
        return;
      }
      const res = await invoke("translate_text", {
        text: trimmed,
        mtSource: mtSource(),
        aiSource: aiSource(),
        side,
        style: style(),
        direction: direction(),
        ticket,
        masterPassword: needsPassword ? masterPassword() : "",
      });
      if (needsPassword) setMasterPassword("");
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
      }
    } catch (err) {
      if (isMt ? ticket !== mtGeneration : ticket !== aiGeneration) return;
      if (isMt) {
        setMtError(String(err));
        setMtStatus("error");
      } else {
        setAiError(String(err));
        setAiStatus("error");
      }
    }
  }

  function queueTranslate(text) {
    saveChain.catch(() => {}).then(() => {
      translateSide("mt", text);
      translateSide("ai", text);
    });
  }

  function onSetting(flag, apply) {
    edited[flag] = true;
    apply();
    const rerun = () => {
      if (!source().trim()) return;
      if (flag === "direction") queueTranslate(source());
      else if (flag === "style" || flag === "aiSource") translateSide("ai", source());
      else if (flag === "mtSource") translateSide("mt", source());
    };
    saveSettings()
      .then(rerun)
      .catch((err) => {
        if (flag === "mtSource" || flag === "direction") {
          setMtError(String(err));
          setMtStatus("error");
        } else if (flag === "aiSource" || flag === "style") {
          setAiError(String(err));
          setAiStatus("error");
        }
      });
  }

  function onKeyInput(flag, apply) {
    edited[flag] = true;
    apply();
  }

  async function onKeyCommit() {
    const name = aiSource() === "deepl" ? "deepl" : aiSource() === "deepseek" ? "deepseek" : "";
    const key = name === "deepl" ? deeplKey().trim() : deepseekKey().trim();
    if (!name || !key) return;
    if (key === (name === "deepl" ? committedDeepl : committedDeepseek)) return;
    if (!masterPassword().trim()) {
      setAiError("请输入主密码。主密码不会保存。");
      setAiStatus("error");
      return;
    }
    try {
      await invoke("save_secret", { name, key, password: masterPassword() });
      if (name === "deepl") {
        committedDeepl = key;
        setHasDeeplKey(true);
        setDeeplKey("");
      } else {
        committedDeepseek = key;
        setHasDeepseekKey(true);
        setDeepseekKey("");
      }
      if (source().trim()) await translateSide("ai", source());
    } catch (err) {
      setAiError(String(err));
      setAiStatus("error");
    }
  }

  function onMasterCommit() {
    if (!masterPassword().trim()) return;
    const key = aiSource() === "deepl" ? deeplKey().trim() : deepseekKey().trim();
    if (key) {
      onKeyCommit();
      return;
    }
    if (source().trim() && (aiSource() === "deepseek" || aiSource() === "deepl")) {
      translateSide("ai", source());
    }
  }

  onMount(() => {
    const unsubs = [];

    invoke("get_settings")
      .then((settings) => {
        if (!settings) return;
        if (!edited.mtSource) setMtSource(settings.mtSource || "google");
        if (!edited.aiSource) setAiSource(settings.aiSource || "free");
        if (!edited.deeplKey) setHasDeeplKey(Boolean(settings.hasDeeplKey));
        if (!edited.deepseekKey) setHasDeepseekKey(Boolean(settings.hasDeepseekKey));
        if (!edited.sourceHeight && settings.sourceHeight) {
          setSourceHeight(clampSourceHeight(settings.sourceHeight));
        }
        if (!edited.direction) setDirection(settings.direction || "auto");
        if (!edited.style) setStyle(settings.style || "none");
      })
      .catch(() => {})
      .finally(() => {
        const shouldSave =
          edited.mtSource ||
          edited.aiSource ||
          edited.deeplKey ||
          edited.deepseekKey ||
          edited.sourceHeight ||
          edited.direction ||
          edited.style;
        settingsLoaded = true;
        if (shouldSave) saveSettings();
        resolveSettingsReady();
      });

    listen("selection-captured", (event) => {
      const text = String(event.payload?.text ?? "");
      setSource(text);
      queueTranslate(text);
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
    }).then((unsub) => unsubs.push(unsub));

    listen("mt-status", (event) => {
      if (event.payload?.ticket !== mtGeneration) return;
      setMtNote(event.payload?.text ?? "");
    }).then((unsub) => unsubs.push(unsub));

    listen("hotkey-error", (event) => {
      setNotice(typeof event.payload === "string" ? event.payload : "快捷键 Ctrl+Alt+T 已被占用。");
    }).then((unsub) => unsubs.push(unsub));

    const onKey = (event) => {
      if (event.key === "Escape") {
        invoke("dismiss");
      }
    };
    window.addEventListener("keydown", onKey);

    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      unsubs.forEach((unsub) => unsub());
    });
  });

  function clampSourceHeight(value) {
    const max = Math.max(48, Math.round(window.innerHeight * 0.6));
    return Math.min(Math.max(Math.round(value) || 72, 48), Math.min(400, max));
  }

  function startSourceResize(event) {
    event.preventDefault();
    edited.sourceHeight = true;
    const startY = event.clientY;
    const startH = sourceHeight();
    const onMove = (moveEvent) => {
      setSourceHeight(clampSourceHeight(startH + moveEvent.clientY - startY));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      saveSettings();
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  function dismissWindow() {
    invoke("dismiss");
  }

  function onType(event) {
    setSource(event.currentTarget.value);
  }

  function onTypeKey(event) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      queueTranslate(source());
    }
  }

  return (
    <div class="panel">
      <div class="bar" data-tauri-drag-region>
        <span class="title" data-tauri-drag-region>
          Vela
        </span>
        <button
          type="button"
          class="close"
          title="关闭"
          aria-label="关闭"
          data-tauri-drag-region="false"
          onClick={dismissWindow}
        >
          ×
        </button>
      </div>

      <textarea
        class="src"
        placeholder="输入中文或英文，回车翻译"
        style={{ height: `${sourceHeight()}px` }}
        value={source()}
        onInput={onType}
        onKeyDown={onTypeKey}
      />
      <Show when={notice()}>
        <p class="banner">{notice()}</p>
      </Show>
      <div
        class="src-split"
        title="拖动调整高度"
        onMouseDown={startSourceResize}
      />

      <div class="engine">
        <select
          aria-label="翻译方向"
          value={direction()}
          onChange={(event) =>
            onSetting("direction", () => setDirection(event.currentTarget.value))
          }
        >
          <option value="auto">自动</option>
          <option value="zh-en">中文 → English</option>
          <option value="en-zh">English → 中文</option>
        </select>
        <select
          aria-label="机器翻译"
          value={mtSource()}
          onChange={(event) =>
            onSetting("mtSource", () => setMtSource(event.currentTarget.value))
          }
        >
          <option value="google">Google</option>
          <option value="mymemory">MyMemory</option>
          <option value="offline">离线</option>
        </select>
      </div>

      <div class="compare">
        <section class="result">
          <Show when={mtStatus() === "idle" && !error()}>
            <p class="empty">也可以选中文字后点蓝色图标。看完后点右上角 ×。</p>
          </Show>
          <Show when={mtStatus() === "loading" && !mtText()}>
            <span class="muted">{mtNote() || "翻译中…"}</span>
          </Show>
          <Show when={mtText()}>{mtText()}</Show>
          <Show when={mtStatus() !== "loading" && mtError()}>
            <span class="err">{mtError()}</span>
          </Show>
          <Show when={error()}>
            <span class="err">{error()}</span>
          </Show>
          <Show when={mtService() === "mymemory" && mtSource() === "google"}>
            <p class="note">已改用 MyMemory</p>
          </Show>
          <Show when={mtService() === "offline" && mtSource() === "google"}>
            <p class="note">已改用离线</p>
          </Show>
        </section>

        <div class="engine">
          <select
            aria-label="风格"
            value={style()}
            onChange={(event) => onSetting("style", () => setStyle(event.currentTarget.value))}
          >
            <option value="none">自然</option>
            <option value="academic">学术</option>
            <option value="colloquial">口语</option>
            <option value="formal">正式</option>
            <option value="idiomatic">地道</option>
            <option value="literal">直译</option>
          </select>
          <select
            aria-label="对照翻译"
            value={aiSource()}
            onChange={(event) =>
              onSetting("aiSource", () => setAiSource(event.currentTarget.value))
            }
          >
            <option value="free">免费测试</option>
            <option value="deepseek">DeepSeek</option>
            <option value="deepl">DeepL</option>
          </select>
          <Show when={aiSource() === "deepseek" || aiSource() === "deepl"}>
            <input
              type="password"
              autocomplete="off"
              placeholder={
                aiSource() === "deepseek"
                  ? hasDeepseekKey()
                    ? "DeepSeek Key 已加密。要更换时再输入"
                    : "DeepSeek API Key"
                  : hasDeeplKey()
                    ? "DeepL Key 已加密。要更换时再输入"
                    : "DeepL API Key"
              }
              value={aiSource() === "deepseek" ? deepseekKey() : deeplKey()}
              onInput={(event) => {
                const value = event.currentTarget.value;
                if (aiSource() === "deepseek") {
                  onKeyInput("deepseekKey", () => setDeepseekKey(value));
                } else {
                  onKeyInput("deeplKey", () => setDeeplKey(value));
                }
              }}
              onBlur={onKeyCommit}
            />
            <input
              type="password"
              autocomplete="off"
              placeholder="主密码，只在这次输入，不保存"
              value={masterPassword()}
              onInput={(event) => setMasterPassword(event.currentTarget.value)}
              onBlur={onMasterCommit}
            />
          </Show>
        </div>

        <section class="result">
          <Show when={aiStatus() === "loading" && !aiText()}>
            <span class="muted">翻译中…</span>
          </Show>
          <Show when={aiText()}>{aiText()}</Show>
          <Show when={aiStatus() !== "loading" && aiError()}>
            <span class="err">{aiError()}</span>
          </Show>
        </section>
      </div>

      <div class="foot">{meta()}</div>
    </div>
  );
}

export default App;
