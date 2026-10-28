# C0 补充核对（只读）

> 版本 0.1.2。逐项回答「已实现 / 部分实现 / 未实现 / 不确定」，附文件、函数名和手动验证步骤。结论依据 `quicktrans/src-tauri/src` 与 `quicktrans/src` 当前代码。

## 1. 打开结果窗的默认动作；是否沿用上一次的动作

**结论：默认「译」；不沿用上次动作，改用「按内容智能推荐」。**

- 选中后实际弹出的是**动作条**（4 按钮 译/释/润/读），不是圆标 Logo（见文末「不一致」第 1 条）。
- 证据：`picker.rs` → `probe_and_show()` → `crate::windows::show_bar()`；推荐由 `picker.rs:recommend()` 调 `suggest.rs::suggest(&hint, Lang::Zh)` 按文字内容算出（单词→释、可编辑纯文→润、其余→译）。
- 点具体按钮：`lib.rs:bar_pick(action)` 把 `action` 透传给结果窗。
- 若走圆标点击路径（`picker.rs` `WM_PICKER_CLICK` → `show_captured` 传 `action: ""`），`App.jsx` 用 `String(event.payload?.action ?? "translate")` 空串回落到「translate」。
- 全仓库无 `lastAction` / `localStorage` / `remember action`，即**不沿用上次动作**。

**手动验证**：先在结果窗切到「润」并关闭；再选一段英文点圆标，应打开「译」。选一个单词与选一整段，动作条高亮可不同。

## 2. 朗读：播放器是否提前释放；找不到语音是否提示

**结论：播放器不会提前释放（阻塞到播完）；找不到对应语言语音时无提示，静默用默认音色。**

- 证据：`speech.rs` `speak()` 用 `Command::output()` 阻塞等 PowerShell 进程退出；脚本内 `$s.Speak($t)` 同步，`SpeechSynthesizer` 活到播完才随进程销毁，不存在提前释放。
- 缺语音：`speech.rs:38-40` `if ($v) { $s.SelectVoice(...) }` —— 找不到匹配音色时**不报错**，直接用系统默认音色。

**手动验证**：在「设置 → 时间和语言 → 语音」里删掉中文语音后朗读中文，听是否用英文腔硬读且无提示。

## 3. 取词失败日志：进程名 / 路径 / 耗时

**结论：部分实现。**

- 证据：`capture/mod.rs` `try_capture()` 用 `eprintln!("[vela-capture] via={via} exe={} ms={}")` 记录了**路径（uia/copy/none）+ 进程名 + 耗时**。
- 缺两点：① 占用剪贴板的进程名只在返回给前端的错误串里（`clipboard.rs:open_holder_name()`），**不写日志**；② `eprintln` 只到 stderr，GUI 无控制台时等于丢弃，**不落盘**。

**手动验证**：命令行运行 `%LOCALAPPDATA%\Vela\vela.exe`，选中文取词，看控制台是否有 `[vela-capture]` 行。

## 4. 同一次选择多次取词：串行化 / 去重

**结论：已实现（拒绝式去重，非排队）。**

- 证据：`capture/mod.rs` `CAPTURE_LOCK: Mutex<()>` + `try_lock()`，抢不到锁返回「上一次取词还没结束」；`lib.rs` 另有 `CAPTURING: AtomicBool` 的 `swap(true)` 门闩，`show_captured`/`on_hotkey` 重复触发直接 return。

**手动验证**：极快连按两次 `Ctrl+Alt+T`，第二次应提示「上一次取词还没结束」而非弹两个窗。

## 5. 圆标阶段（未点动作前）是否读写剪贴板

**结论：不读写剪贴板。**

- 证据：`picker.rs:190` 注释「Only probe with UI Automation — never touch the clipboard on a selection」；`probe_and_show()` 只调 `probe_uia_only()`（UIA 只读），`show_bar` 无剪贴板操作。真正动剪贴板在点按钮后的 `copykey::copykey_selection()`。

**手动验证**：复制内容 A 到剪贴板 → 选中另一段文字但不点按钮 → 去别处 `Ctrl+V`，粘贴出的仍是 A（未被污染）。

## 6. 点「替换」是否先确认目标窗口 / 输入框仍是原来的

**结论：部分实现（窗口/进程有验证，输入框无实时验证）。**

- 证据：`lib.rs:replace_text` → `writeback::write_ready` → `target::switch_back_and_verify()`（`writeback/target.rs:58`）：先 `SetForegroundWindow` 切回，再 300ms 内确认前台 `hwnd` 或 `pid` 与记录一致，否则报「无法切回原来的窗口」。
- 但「输入框」是否仍是可编辑的那一个：只靠 `capture/mod.rs:is_editable_exe`（进程名白名单）在**取词时一次性**判断，写回时**没有**再做 UIA ValuePattern 实时校验；进程相同但焦点已变时会写错位置。

**手动验证**：选中→翻译→点「替换」前先点同窗口另一输入框，观察写回位置。

## 7. 润色：力度档位 / 逐词对比 / 「为什么这样改」

**结论：三样均已实现。**

- 力度档位：`App.jsx` `polishLevel` 三档 light/standard/heavy（`···` 菜单），`translate.rs:polish_prompt` 按档生成不同 prompt。
- 逐词对比：`App.jsx:diffWords()` 词级 diff（del/add/same）。
- 「为什么这样改」：`lib.rs:explain_diff` 命令 + `App.jsx:explainDiff()` → `translate.rs:explain_diff_prompt`。

**手动验证**：润色后看逐词高亮；点「为什么这样改」看 AI 说明。

## 8. 不弹圆标名单；游戏、终端是否误弹

**结论：未实现屏蔽名单；终端、游戏会弹（无全屏/游戏检测）。**

- 证据：全仓库只有 `is_editable_exe`（可编辑白名单）、`picker.rs:foreground_is_us()`（跳过自身）、`probe_and_show` 对 `PasswordField` 跳过。**没有**「不弹动作条」的应用黑名单，也无全屏/游戏窗口检测。
- 终端：`copykey.rs:is_console` 只是把取词改 `Ctrl+Insert`，动作条**照样弹**；全屏独占游戏可能被鼠标钩子触发误弹。

**手动验证**：打开全屏游戏或终端选中文字，看动作条是否弹出。

## 9. 完全本地模式下是否还有外网请求

**结论：未完全本地——离线翻译首次使用会从外网下载模型。**

- 证据：`lib.rs:106-110` `local_only` 强制 `mt=offline + ai=ollama`，Ollama 走 localhost；但 `offline.rs:ensure_models` 在模型缺失时从 `huggingface.co`（失败回退 `hf-mirror.com`）下载 encoder/decoder/tokenizer。全仓库**无**更新检查、遥测、字体下载。

**手动验证**：删 `%LOCALAPPDATA%\com.vela.desktop\models` → 开完全本地模式 → 翻译，用资源监视器看是否有 huggingface 连接。

## 10. 除母语外硬编码中英文的位置

| 文件:行 | 内容 |
| --- | --- |
| `translate.rs:874-879` | `target_lang()` 仅「有汉字→en，否则→zh-CN」两语言 |
| `translate.rs:621,623` | `strict_translation_prompt` 硬编码 "English" / "Simplified Chinese" |
| `translate.rs:632,634` | `translation_prompt` 同上两语言 |
| `translate.rs:660` | `explanation_prompt` 固定「用简体中文解释」 |
| `translate.rs:689` | `explain_diff_prompt` 固定「用简体中文简要说明」 |
| `translate.rs:882-888` | `direction_label` 固定「中文 → English / English → 中文」 |
| `speech.rs:17-19` | 语音语言选择仅 `is_cjk→"zh"` 否则 `"en"` |
| `suggest.rs:220` | 母语硬编码 `Lang::Zh` |
| `offline.rs:56-57` | OPUS-MT 模型键仅 en-zh / zh-en 两个方向 |

---

## 本轮核对发现的与文档/预期不一致

1. **README 说「圆标」，代码里圆标 Logo 是死代码。** `picker.rs` 的 `show_icon()`（真正的圆形 Logo）全文件无调用点，`VISIBLE` 永不置 true，`WM_PICKER_CLICK` 路径实际不可达。现在选中后出现的是**动作条**（译/释/润/读四按钮）。README「圆标」描述已过时。

2. **「完全本地模式」名称与行为不符。** 文字不外发，但首次离线翻译会联网下载模型文件（huggingface/hf-mirror）。严格说首次不是「完全本地」。

3. **取词日志无落盘。** `eprintln` 只进 stderr，GUI 双击运行时看不到；且不记录占用剪贴板的进程名。

4. **缺语音静默降级。** 找不到匹配语言 TTS 音色时不提示，直接用默认音色硬读。

5. **可编辑判断是静态进程名。** 「替换」是否可用只看 `is_editable_exe` 白名单，写回时不做实时焦点/输入框校验。
