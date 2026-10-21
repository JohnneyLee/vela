# Vela

在 Windows 上选中文字后，旁边会出现一个图标。点击图标，即可在中文和英文之间翻译。按 `Ctrl+Alt+T` 也会翻译当前选中的文字。打开窗口后，也可以直接打字，按回车翻译。译文上下排列，两边各自请求。换上面的来源只重查上面，换下面的来源只重查下面。方向可以选自动、中文到英文或英文到中文。下面的风格只影响 DeepL、DeepSeek 和免费测试。

上面可选 Google、MyMemory 或离线模型。选 Google 时，如果 Google 和 MyMemory 都不通，会改用离线模型。离线模型第一次使用时需要联网下载，下载完成后可以离线翻译。下面可选免费测试、DeepSeek 或 DeepL。免费测试和 DeepSeek 会边生成边显示；关掉窗口或换来源会取消还没完成的请求。DeepSeek 和 DeepL 的 Key 存在 Windows 凭据管理器里，不写进设置文件，也不写进代码。

如果 `Ctrl+Alt+T` 已经被别的软件占用，窗口会提示。以管理员身份运行的窗口里，普通程序发不出复制按键，选中后可能没有图标。安装包没有代码签名，Windows 可能会给出未知发布者提示。更新时重新安装新的安装包。

点窗口上的 ×，或按 `Esc`，会隐藏窗口并清掉这一次的结果。托盘菜单中的「退出」会退出程序。「开机启动」是可选项：勾选后，登录 Windows 时会自动打开 Vela。

## 开发

在项目目录中执行：

```text
npm install
npm run tauri dev
```

## 安装包

```text
npm run tauri build
```

NSIS 安装包位于 `src-tauri/target/release/bundle/nsis/`。
