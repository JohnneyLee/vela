# Vela

Windows 划词翻译工具。选中文字后，窗口贴在光标旁边，翻译、解释、润色或朗读，看完即关，不记住译文。

Vela is a Windows selection translator. Select text and a small window appears next to the cursor to translate, explain, polish, or read it aloud. It closes when you are done and does not remember the result.

## 下载

从 [GitHub Releases](https://github.com/JohnneyLee/vela/releases) 下载后双击安装即可，不需要编译。

需要 Windows 10 或 Windows 11。Windows 11 自带 WebView2；Windows 10 若提示缺少 WebView2，按提示安装即可。程序没有代码签名，第一次运行时 Windows SmartScreen 可能提示「未知发布者」，点「仍要运行」即可。

## Download

Version **0.1.2**, installer `Vela_0.1.2_x64-setup.exe` (NSIS, ~8 MB). Download it from [GitHub Releases](https://github.com/JohnneyLee/vela/releases) and double-click to install; no build step.

Windows 10 or 11. Windows 11 includes WebView2. The program is unsigned, so SmartScreen may show an unknown-publisher warning the first time; choose "Run anyway".

## 功能

- **取词翻译**：选中文字，旁边出现操作条（译 / 释 / 润 / 读）。程序按内容自动推荐高亮哪一项：单词推荐「释」，可编辑文本推荐「润」，其余推荐「译」。
- **四类操作**：翻译、解释（含词性/音标/例句）、润色（三档力度，逐词对比，可问「为什么这样改」）、朗读。
- **机器翻译**：Google、MyMemory、DeepL、离线（OPUS-MT）。
- **AI 翻译**：免费公共服务、DeepSeek、OpenAI、Moonshot Kimi、Google Gemini、Ollama（本机）。
- **对照显示**：并排对照 / AI 优先 / 机器翻译优先 / 自动，四档可切换。
- **写回与撤销**：在可编辑的窗口里，可把译文替换回原文处，并支持撤销。
- **主题**：跟随系统 / 浅色 / 深色 / 护眼绿 / 暖色米黄。
- **母语设置**：中文 / English / 日本語，决定翻译方向（原文是母语时译成英文，否则译成母语）。
- **完全本地模式**：只用离线翻译和 Ollama，不把文字发到任何外网服务。

## 使用

- **划词**：选中文字，点操作条上的按钮（或直接按快捷键）打开结果。
- **快捷键**：`Ctrl+Alt+T` 取当前选中文字。
- **翻译剪贴板**：`Ctrl+Alt+Shift+T` 读取一次剪贴板里的文字并翻译（可在设置里改这个快捷键）。
- **手动输入**：在结果窗里直接打字，回车翻译。
- **关闭**：点 × 或按 `Esc`，清掉这一轮的原文和译文。

## 隐私

原文和译文都不写入磁盘、不写进日志。文字会发送到对应的翻译服务：

| 服务 | 文字发往 | 是否需要 Key |
| --- | --- | --- |
| Google 机器翻译 | translate.googleapis.com | 否 |
| MyMemory | api.mymemory.translated.net | 否 |
| DeepL | api.deepl.com | 是 |
| 免费 AI | text.pollinations.ai | 否 |
| DeepSeek | api.deepseek.com | 是 |
| OpenAI | api.openai.com | 是 |
| Moonshot Kimi | api.moonshot.cn | 是 |
| Google Gemini | generativelanguage.googleapis.com | 是 |
| Ollama | 本机 localhost:11434 | 否 |
| 离线翻译 | 本机（首次使用会从 Hugging Face 下载模型） | 否 |

- **完全本地模式**：只使用离线翻译和 Ollama，不发送任何文字到外网。注意：离线模型第一次使用时需要联网下载一次，之后留在本机。
- **API Key**：用 Windows 的加密接口（DPAPI）保存在本机，只和你当前的 Windows 账户绑定，不写进设置文件，也不出现在日志里。

## 已知限制

- 程序没有代码签名，会触发 SmartScreen 警告。
- 暂无截图翻译（OCR）；遇到不支持直接取词的软件，可以手动复制文字，再按 `Ctrl+Alt+Shift+T`。
- 以管理员权限运行的窗口里，普通权限的 Vela 无法发送复制按键，取不到词；可以给 Vela 管理员权限，或手动复制后翻译剪贴板。

## 从源码构建

需要 Node.js 和 Rust。

```text
npm install
npm run tauri dev
```

安装包：

```text
npm run tauri build
```

NSIS 安装包生成在 `src-tauri/target/release/bundle/nsis/`。

## Build from source

Node.js and Rust are required.

```text
npm install
npm run tauri dev
```

Installer:

```text
npm run tauri build
```

The NSIS installer is written to `src-tauri/target/release/bundle/nsis/`.

## 许可

[MIT](LICENSE)
