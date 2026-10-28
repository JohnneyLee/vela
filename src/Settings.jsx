import { createSignal, onCleanup, onMount, Show, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./tokens.css";
import "./settings.css";

// AI providers. "free" and "ollama" need no key. Cloud providers show the
// domain their requests go to; local ones say the text never leaves the machine.
const AI_PROVIDERS = [
  { id: "free", label: "免费（公共服务）", cloud: true, needsKey: false, domain: "text.pollinations.ai" },
  { id: "ollama", label: "Ollama（本机）", cloud: false, needsKey: false, domain: "" },
  { id: "deepseek", label: "DeepSeek", cloud: true, needsKey: true, domain: "api.deepseek.com" },
  { id: "openai", label: "OpenAI", cloud: true, needsKey: true, domain: "api.openai.com" },
  { id: "moonshot", label: "Moonshot Kimi", cloud: true, needsKey: true, domain: "api.moonshot.cn" },
  { id: "gemini", label: "Google Gemini", cloud: true, needsKey: true, domain: "generativelanguage.googleapis.com" },
];

const DISPLAY_MODES = [
  { id: "compare", label: "并排对照（推荐）", note: "机器翻译和 AI 结果并排展示，方便对比。" },
  { id: "ai-first", label: "AI 优先", note: "AI 结果是主结果，机器翻译折叠成一行。" },
  { id: "mt-first", label: "机器翻译优先", note: "机器翻译是主结果，AI 折叠成一行。" },
  { id: "auto", label: "自动", note: "有 Key 时默认 AI 优先，否则机器翻译优先。" },
];

function displayNote(id) {
  return (DISPLAY_MODES.find((m) => m.id === id) || DISPLAY_MODES[0]).note;
}

function provider(id) {
  return AI_PROVIDERS.find((p) => p.id === id);
}

// 把后端存的小写快捷键转成 "Ctrl+Alt+Shift+T" 显示格式。
function displayShortcut(raw) {
  const parts = (raw || "").split("+").filter(Boolean);
  if (parts.length === 0) return "";
  return parts
    .map((p) => {
      const t = p.trim().toLowerCase();
      if (t === "ctrl" || t === "control") return "Ctrl";
      if (t === "alt") return "Alt";
      if (t === "shift") return "Shift";
      if (t === "super" || t === "win" || t === "meta") return "Win";
      return t.toUpperCase();
    })
    .join("+");
}

function Settings() {
  const [native, setNative] = createSignal("zh");
  const [clipboardShortcut, setClipboardShortcut] = createSignal("Ctrl+Alt+Shift+T");
  const [screenshotShortcut, setScreenshotShortcut] = createSignal("Ctrl+Alt+S");
  const [theme, setTheme] = createSignal("system");
  const [mtSource, setMtSource] = createSignal("google");
  const [aiSource, setAiSource] = createSignal("free");
  const [style, setStyle] = createSignal("none");
  const [displayMode, setDisplayMode] = createSignal("compare");
  const [allowPartialClipboard, setAllowPartialClipboard] = createSignal(false);
  const [localOnly, setLocalOnly] = createSignal(false);
  const [autostart, setAutostart] = createSignal(false);
  const [keys, setKeys] = createSignal({});
  const [drafts, setDrafts] = createSignal({});
  const [editingKey, setEditingKey] = createSignal({});
  // editId: which provider's details are shown; aiSource: actually in use.
  const [editId, setEditId] = createSignal("free");
  const [addOpen, setAddOpen] = createSignal(false);
  const [advancedOpen, setAdvancedOpen] = createSignal(false);
  const [toast, setToast] = createSignal("");
  const [testing, setTesting] = createSignal({});
  const [testResult, setTestResult] = createSignal({});
  const [ollamaModels, setOllamaModels] = createSignal([]);
  const [ollamaModel, setOllamaModel] = createSignal("");
  let toastTimer = 0;

  function showToast(text) {
    setToast(text);
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => setToast(""), 2400);
  }

  function hasKey(id) {
    return Boolean(keys()[id]);
  }

  function draft(id) {
    return drafts()[id] || "";
  }

  function setDraft(id, value) {
    setDrafts((prev) => ({ ...prev, [id]: value }));
  }

  function save(partial) {
    invoke("save_settings", {
      settings: {
        mtSource: mtSource(),
        aiSource: aiSource(),
        style: style(),
        displayMode: displayMode(),
        allowPartialClipboard: allowPartialClipboard(),
        localOnly: localOnly(),
        ollamaModel: ollamaModel(),
        theme: theme(),
        native: native(),
        clipboardShortcut: clipboardShortcut().toLowerCase().replace(/\s+/g, ""),
        screenshotShortcut: screenshotShortcut().toLowerCase().replace(/\s+/g, ""),
        ...partial,
      },
    })
      .then(() => showToast("✓ 已更新"))
      .catch((err) => showToast(String(err)));
  }

  onMount(() => {
    invoke("get_settings")
      .then((s) => {
        if (!s) return;
        setMtSource(s.mtSource || "google");
        setAiSource(s.aiSource || "free");
        setEditId(s.aiSource || "free");
        setStyle(s.style || "none");
        setDisplayMode(s.displayMode || "compare");
        setAllowPartialClipboard(Boolean(s.allowPartialClipboard));
        setLocalOnly(Boolean(s.localOnly));
        setOllamaModel(s.ollamaModel || "");
        setTheme(s.theme || "system");
        setNative(s.native || "zh");
        setClipboardShortcut(displayShortcut(s.clipboardShortcut || "ctrl+alt+shift+t"));
        setScreenshotShortcut(displayShortcut(s.screenshotShortcut || "ctrl+alt+s"));
        setKeys({
          deepseek: s.hasDeepseekKey,
          deepl: s.hasDeeplKey,
          openai: s.hasOpenaiKey,
          moonshot: s.hasMoonshotKey,
          gemini: s.hasGeminiKey,
        });
        // 当前用的是 Ollama 时，进来就拉一次模型列表，让模型下拉框可用。
        if (s.aiSource === "ollama") loadOllama();
      })
      .catch(() => {});

    const onKey = (e) => {
      if (e.key === "Escape") invoke("settings_close");
    };
    window.addEventListener("keydown", onKey);

    // 开机启动：独立于 settings.json，由插件管理。读初值并监听托盘改动。
    invoke("get_autostart").then((v) => setAutostart(Boolean(v))).catch(() => {});
    const unsubAutostart = listen("autostart-changed", (e) => setAutostart(Boolean(e.payload)));

    onCleanup(() => {
      window.removeEventListener("keydown", onKey);
      window.clearTimeout(toastTimer);
      unsubAutostart.then((u) => u());
    });
  });

  // —— 已添加 = free + ollama + 已配置 key 的云端服务 ——
  const added = () => AI_PROVIDERS.filter((p) => !p.needsKey || hasKey(p.id));
  const notAdded = () => AI_PROVIDERS.filter((p) => p.needsKey && !hasKey(p.id));

  function onEditChange(e) {
    const id = e.currentTarget.value;
    if (id === "__add__") {
      setAddOpen(true);
      return;
    }
    // 下拉框只列已配置的服务，选中即真正切换。
    setAiSource(id);
    setEditId(id);
    save({ aiSource: id });
    if (id === "ollama" && ollamaModels().length === 0) loadOllama();
  }

  function addProvider(id) {
    setAddOpen(false);
    setEditId(id);
    setDraft(id, "");
    // Not configured yet: details show the form; the in-use engine stays.
  }

  function startEditKey(id) {
    setEditingKey((prev) => ({ ...prev, [id]: true }));
    setDraft(id, "");
  }

  async function saveAndTest(id) {
    const k = draft(id).trim();
    const p = provider(id);
    if (!k && !hasKey(id)) {
      showToast("请先填写 Key");
      return;
    }
    if (k) {
      try {
        await invoke("save_key", { name: id, key: k });
        setKeys((prev) => ({ ...prev, [id]: true }));
        setEditingKey((prev) => ({ ...prev, [id]: false }));
        setDraft(id, "");
      } catch (err) {
        showToast(String(err));
        return;
      }
    }
    setTesting((prev) => ({ ...prev, [id]: true }));
    setTestResult((prev) => ({ ...prev, [id]: "" }));
    try {
      const res = await invoke("test_ai_connection", { source: id });
      setTestResult((prev) => ({ ...prev, [id]: res }));
      if (k) showToast("✓ 已配置");
      // Now configured: if this is the selected AI provider, actually switch.
      if (p && p.needsKey && editId() === id) {
        setAiSource(id);
        save({ aiSource: id });
      }
    } catch (err) {
      setTestResult((prev) => ({ ...prev, [id]: String(err) }));
    } finally {
      setTesting((prev) => ({ ...prev, [id]: false }));
    }
  }

  async function removeService(id) {
    try {
      await invoke("delete_key", { name: id });
    } catch (err) {
      showToast(String(err));
      return;
    }
    setKeys((prev) => ({ ...prev, [id]: false }));
    setEditingKey((prev) => ({ ...prev, [id]: false }));
    if (aiSource() === id || editId() === id) {
      setAiSource("free");
      setEditId("free");
      save({ aiSource: "free" });
    }
    showToast("已移除");
  }

  function applyModels(models) {
    const list = models || [];
    setOllamaModels(list);
    // 未选过模型时，默认用第一个已安装模型。
    if (!ollamaModel() && list.length > 0) {
      setOllamaModel(list[0]);
      save({ ollamaModel: list[0] });
    }
  }

  function loadOllama() {
    invoke("detect_ollama")
      .then((models) => {
        applyModels(models);
        setTestResult((prev) => ({ ...prev, ollama: `✓ 已连接 · ${(models || []).length} 个模型` }));
      })
      .catch((err) => setTestResult((prev) => ({ ...prev, ollama: String(err) })));
  }

  function testOllama() {
    setTesting((prev) => ({ ...prev, ollama: true }));
    setTestResult((prev) => ({ ...prev, ollama: "" }));
    invoke("detect_ollama")
      .then((models) => {
        applyModels(models);
        setTestResult((prev) => ({ ...prev, ollama: `✓ 已连接 · ${(models || []).length} 个模型` }));
      })
      .catch((err) => setTestResult((prev) => ({ ...prev, ollama: String(err) })))
      .finally(() => setTesting((prev) => ({ ...prev, ollama: false })));
  }

  function mtNote() {
    switch (mtSource()) {
      case "offline":
        return "文字只在本机处理。";
      case "deepl":
        return "文字会发送到 api.deepl.com。";
      case "mymemory":
        return "文字会发送到 api.mymemory.translated.net。";
      default:
        return "文字会发送到 translate.googleapis.com。";
    }
  }

  const current = () => provider(editId());

  return (
    <div class="settings">
      <Show when={toast()}>
        <div class="toast">{toast()}</div>
      </Show>

      <section class="group">
        <h2>通用</h2>
        <label class="row">
          <span>母语</span>
          <select value={native()} onChange={(e) => { setNative(e.currentTarget.value); save({ native: e.currentTarget.value }); }}>
            <option value="zh">中文</option>
            <option value="en">English</option>
            <option value="ja">日本語</option>
          </select>
        </label>
        <label class="row">
          <span>主题</span>
          <select
            value={theme()}
            onChange={(e) => {
              setTheme(e.currentTarget.value);
              save({ theme: e.currentTarget.value });
            }}
          >
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
            <option value="green">护眼绿</option>
            <option value="sepia">暖色米黄</option>
          </select>
        </label>
        <label class="row">
          <span>开机启动</span>
          <input
            type="checkbox"
            class="switch"
            checked={autostart()}
            onChange={(e) => {
              const want = e.currentTarget.checked;
              setAutostart(want);
              invoke("set_autostart", { enabled: want })
                .then((actual) => setAutostart(Boolean(actual)))
                .catch((err) => {
                  showToast(String(err));
                  invoke("get_autostart").then((v) => setAutostart(Boolean(v))).catch(() => {});
                });
            }}
          />
        </label>
        <label class="row">
          <span>翻译剪贴板快捷键</span>
          <input
            type="text"
            class="shortcut-input"
            autocomplete="off"
            spellcheck={false}
            placeholder="Ctrl+Alt+Shift+T"
            value={clipboardShortcut()}
            onInput={(e) => setClipboardShortcut(e.currentTarget.value)}
            onBlur={() => save({ clipboardShortcut: clipboardShortcut().toLowerCase().replace(/\s+/g, "") })}
          />
        </label>
        <label class="row">
          <span>截图翻译快捷键</span>
          <input
            type="text"
            class="shortcut-input"
            autocomplete="off"
            spellcheck={false}
            placeholder="Ctrl+Alt+S"
            value={screenshotShortcut()}
            onInput={(e) => setScreenshotShortcut(e.currentTarget.value)}
            onBlur={() => save({ screenshotShortcut: screenshotShortcut().toLowerCase().replace(/\s+/g, "") })}
          />
        </label>
        <p class="note">复制文字后按这个快捷键，直接翻译剪贴板里的内容。取词不到时也可用这种方式。</p>
        <p class="note">按截图翻译快捷键框选屏幕区域，识别文字后翻译并盖回原位。</p>
      </section>

      <section class="group">
        <h2>翻译</h2>
        <label class="row">
          <span>机器翻译</span>
          <select value={mtSource()} onChange={(e) => { setMtSource(e.currentTarget.value); save(); }}>
            <option value="google">Google</option>
            <option value="mymemory">MyMemory</option>
            <option value="deepl">DeepL</option>
            <option value="offline">离线</option>
          </select>
        </label>
        <Show when={mtSource() === "deepl"}>
          <div class="row">
            <span>DeepL Key</span>
            {hasKey("deepl") && !editingKey().deepl ? (
              <button type="button" class="link-btn" onClick={() => startEditKey("deepl")}>
                已保存 · 更换
              </button>
            ) : (
              <input
                type="password"
                autocomplete="off"
                placeholder="DeepL API Key"
                value={draft("deepl")}
                onInput={(e) => setDraft("deepl", e.currentTarget.value)}
              />
            )}
          </div>
          <div class="row">
            <span></span>
            <span class="key-actions">
              <button type="button" class="primary-btn" disabled={testing().deepl} onClick={() => saveAndTest("deepl")}>
                {testing().deepl ? "测试中…" : "保存并测试"}
              </button>
              <Show when={hasKey("deepl")}>
                <button type="button" class="link-btn danger" onClick={() => removeService("deepl")}>
                  移除此服务
                </button>
              </Show>
            </span>
          </div>
        </Show>
        <p class="note">{mtNote()}</p>
        <label class="row">
          <span>风格</span>
          <select value={style()} onChange={(e) => { setStyle(e.currentTarget.value); save(); }}>
            <option value="none">自然</option>
            <option value="academic">学术</option>
            <option value="colloquial">口语</option>
            <option value="formal">正式</option>
            <option value="idiomatic">地道</option>
            <option value="literal">直译</option>
          </select>
        </label>
        <label class="row">
          <span>结果显示方式</span>
          <select value={displayMode()} onChange={(e) => { setDisplayMode(e.currentTarget.value); save(); }}>
            {DISPLAY_MODES.map((m) => (
              <option value={m.id}>{m.label}</option>
            ))}
          </select>
        </label>
        <p class="note">{displayNote(displayMode())}</p>
      </section>

      <section class="group">
        <h2>AI 翻译</h2>
        <label class="row">
          <span>使用</span>
          <select value={aiSource()} onChange={onEditChange}>
            <For each={added()}>
              {(p) => <option value={p.id}>{p.label}</option>}
            </For>
            <option value="__add__">＋ 添加服务…</option>
          </select>
        </label>

        {/* 未配置服务的提示 */}
        <Show when={current() && current().needsKey && !hasKey(editId())}>
          <p class="note warn">填好 Key 并测试通过后才会启用。</p>
        </Show>

        {/* 详情 */}
        <div class="engine-panel">
          <Show when={current() && current().needsKey}>
            <div class="row">
              <span>Key</span>
              {hasKey(editId()) && !editingKey()[editId()] ? (
                <button type="button" class="link-btn" onClick={() => startEditKey(editId())}>
                  已保存 · 更换
                </button>
              ) : (
                <input
                  type="password"
                  autocomplete="off"
                  placeholder={`${current().label} API Key`}
                  value={draft(editId())}
                  onInput={(e) => setDraft(editId(), e.currentTarget.value)}
                />
              )}
            </div>
            <p class="note">Key 加密保存在本机。文字会发送到 {current().domain}。</p>
            <div class="row">
              <span></span>
              <span class="key-actions">
                <button type="button" class="primary-btn" disabled={testing()[editId()]} onClick={() => saveAndTest(editId())}>
                  {testing()[editId()] ? "测试中…" : "保存并测试"}
                </button>
                <Show when={hasKey(editId())}>
                  <button type="button" class="link-btn danger" onClick={() => removeService(editId())}>
                    移除此服务
                  </button>
                </Show>
              </span>
            </div>
            <Show when={testResult()[editId()]}>
              <p class="note">{testResult()[editId()]}</p>
            </Show>
          </Show>

          <Show when={current() && current().id === "ollama"}>
            <div class="row">
              <span>地址</span>
              <code class="addr">http://localhost:11434</code>
            </div>
            <div class="row">
              <span>模型</span>
              <select
                value={ollamaModel()}
                disabled={ollamaModels().length === 0}
                onChange={(e) => {
                  setOllamaModel(e.currentTarget.value);
                  save({ ollamaModel: e.currentTarget.value });
                }}
              >
                <Show when={ollamaModels().length > 0} fallback={<option>未检测到 Ollama</option>}>
                  <For each={ollamaModels()}>{(m) => <option value={m}>{m}</option>}</For>
                </Show>
              </select>
            </div>
            <p class="note">文字只在本机处理。未安装时访问 <a href="https://ollama.com">ollama.com</a>。</p>
            <div class="row">
              <span></span>
              <span class="key-actions">
                <button type="button" class="primary-btn" disabled={testing().ollama} onClick={testOllama}>
                  {testing().ollama ? "测试中…" : "测试连接"}
                </button>
              </span>
            </div>
            <Show when={testResult().ollama}>
              <p class="note">{testResult().ollama}</p>
            </Show>
          </Show>

          <Show when={current() && current().id === "free"}>
            <p class="note">速度和稳定性不保证，文字会发送到 text.pollinations.ai。</p>
          </Show>
        </div>

        {/* 添加服务 */}
        <Show when={addOpen()}>
          <div class="add-list">
            <For each={notAdded()}>
              {(p) => (
                <button type="button" class="add-item" onClick={() => addProvider(p.id)}>
                  {p.label}
                </button>
              )}
            </For>
          </div>
        </Show>
      </section>

      <section class="group">
        <button type="button" class="advanced-toggle" onClick={() => setAdvancedOpen(!advancedOpen())} aria-expanded={advancedOpen()}>
          {advancedOpen() ? "▾" : "▸"} 高级
        </button>
        <Show when={advancedOpen()}>
          <label class="row">
            <span>完全本地模式</span>
            <input
              type="checkbox"
              class="switch"
              checked={localOnly()}
              onChange={(e) => { setLocalOnly(e.currentTarget.checked); save({ localOnly: e.currentTarget.checked }); }}
            />
          </label>
          <p class="note">开启后只使用本机服务（离线翻译 + Ollama），不发送任何文字。</p>

          <label class="row">
            <span>剪贴板有无法备份的内容时仍取词</span>
            <input
              type="checkbox"
              class="switch"
              checked={allowPartialClipboard()}
              onChange={(e) => { setAllowPartialClipboard(e.currentTarget.checked); save({ allowPartialClipboard: e.currentTarget.checked }); }}
            />
          </label>
          <p class="note">可能丢失剪贴板内容，默认关闭。</p>
        </Show>
      </section>
    </div>
  );
}

export default Settings;
