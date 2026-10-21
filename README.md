# Vela

选中文字，或直接输入。窗口贴在光标旁边，看完即关，不记住译文。

Select text, or type it. The window sits beside the cursor, then closes and forgets the result.

## 下载

[Vela 0.1.0](https://github.com/JohnneyLee/vela/releases/latest) 里的 `vela.exe` 下载后即可运行，不必编译。

需要 Windows 10 或 Windows 11。Windows 11 已带 WebView2。程序没有代码签名，第一次打开时 Windows 可能提示未知发布者。

## Download

Download `vela.exe` from [Vela 0.1.0](https://github.com/JohnneyLee/vela/releases/latest) and run it. No build step.

Windows 10 or 11. Windows 11 already includes WebView2. The executable is unsigned, so Windows may show an unknown-publisher prompt the first time.

## 使用

选中文字后，旁边出现一个圆标。点击它，或按 `Ctrl+Alt+T`。也可以在窗口里打字，按回车翻译。

译文分上下两栏，各自请求。上面是机器翻译：Google、MyMemory、离线。下面是对照：免费测试、DeepSeek、DeepL。只更换一边时，另一边不会重查。Google 和 MyMemory 都不通时，上面会改用离线模型。离线模型第一次使用时需要下载，之后留在本机。

DeepSeek 和 DeepL 需要你自己的 Key。Key 和主密码一起输入后加密保存，主密码不会写入文件。点 × 或按 `Esc` 关闭窗口，并清掉这一次的原文和译文。托盘菜单可以退出，也可以勾选开机启动。

以管理员身份运行的窗口里，普通程序发不出复制按键，选中后可能没有圆标。

## Use

After you select text, a round icon appears beside it. Click the icon, or press `Ctrl+Alt+T`. You can also type in the window and press Enter.

The result is two independent rows. The top row is machine translation: Google, MyMemory, or offline. The bottom row is a comparison: a free test model, DeepSeek, or DeepL. Changing one row does not rerun the other. If Google and MyMemory both fail, the top row falls back to the offline model. The offline model downloads on first use and then stays on the computer.

DeepSeek and DeepL use your own key. The key is encrypted with a master password, and that password is not stored. Close the window with × or `Esc`. That clears the text from this turn. The tray menu can quit the app or turn on launch at login.

In a window running as administrator, a normal program cannot send the copy keystroke, so the icon may not appear.

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

NSIS 安装包在 `src-tauri/target/release/bundle/nsis/`。

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

## License

[MIT](LICENSE)
