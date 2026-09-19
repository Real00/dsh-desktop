# DSH Desktop

开箱即用的 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（`dsh`）桌面壳，基于 **Tauri 2**。

打开应用后会自动：

1. 检测本机 `dsh` 或 `npx`
2. **首次启动**安装默认插件（见下）
3. 启动 `dsh web`（随机本机端口）
4. 等待就绪后在窗口内打开官方 Web UI
5. 退出时结束子进程

> 本项目**不重写** DSH Web UI，只做原生窗口托管与进程管理。

## 前置条件

- Windows 10+ / macOS 11+ Apple Silicon / Linux x64（及可选 aarch64）
- CI：Windows NSIS、macOS arm64、Linux AppImage/deb；不做 Intel Mac
- [Node.js](https://nodejs.org/) **≥ 22.19**（运行时用来拉起 `@deepseek-ai/dsh`）
- 开发还需要：Rust stable、系统 WebView 依赖（见 [Tauri prerequisites](https://tauri.app/start/prerequisites/)）


## 本地托管运行时（不再每次 npx）

应用把 `@deepseek-ai/dsh` 安装到：

```text
~/.dsh-desktop/runtime/
```

- **首次启动**：`npm install @deepseek-ai/dsh@<pinned>` 到该目录（只需一次）
- **之后启动**：直接 `node …/node_modules/@deepseek-ai/dsh/lib/bin.js web`，**不再走 npx**
- **更新**：启动页提示新版本，或调用「更新 dsh 运行时」（等价于在该目录 `npm install @deepseek-ai/dsh@latest`）

> 安装包体积仍保持精简：不把整份 Node/`node_modules` 打进 `.dmg`（那会非常大且难签名）。  
> 运行时缓存在用户目录，跨应用升级保留；需要的话可在后续版本增加「从 Release 预置 runtime 缓存」加速首次安装。

就绪检测会等到 **HTTP 返回真实页面内容** 再导航进 Web UI，减轻白屏。

## 本地开发

```bash
cd dsh-desktop
npm install
npm run tauri dev
```

仅构建前端：

```bash
npm run build
```

本地打 Windows 包：

```bash
npm run tauri build
# 产物通常在 src-tauri/target/release/bundle/nsis
```

## GitHub Actions

| 文件 | 作用 |
| --- | --- |
| `.github/workflows/ci.yml` | PR / push 时构建前端 |
| `.github/workflows/release-windows.yml` | 打 **Windows NSIS** 安装包 |
| `.github/workflows/release-macos.yml` | 打 **Apple Silicon** `.dmg` / `.app`（不做 Intel） |
| `.github/workflows/release-linux.yml` | 打 **Linux** AppImage + deb（x64；aarch64 best-effort） |

触发方式：

- 推送标签：`git tag v0.1.3 && git push origin v0.1.3`
- 或在 Actions 里手动 `workflow_dispatch`

产物通过 `tauri-apps/tauri-action` 发到 **Draft Release**（Windows `.exe` / NSIS，macOS arm64 `.dmg`）。

## 默认插件与首次向导

首次启动（或 `settings.json` 中 `wizardCompleted` 未设置）会显示**插件向导**，推荐勾选：

| 插件 | 作用 |
| --- | --- |
| `dshmarket` | 应用内插件市场 |
| `loopx` | 长任务 Goal / Todo / 配额控制面 |
| `dsh-chat-import` | 对话导入 |
| `dsh-llm-capabilities` | 模型能力探测 |

可「使用推荐」「安装所选」或「跳过」。完成后只安装**所选**集合中缺失的插件。可在启动页「桌面设置 → 打开插件向导」重新打开。

已安装检测：`~/.dsh/profiles/web/package.json` 关键词，或 `~/.dsh-desktop/bootstrap-plugins.json`。安装失败不会阻止启动。

## 桌面设置（启动页）

- 窗口位置/大小记忆（`tauri-plugin-window-state`）
- 开机启动、全局快捷键（默认 `CommandOrControl+Shift+D`，默认关闭）
- CLI shim：`~/.local/bin/dsh`（Unix）或 `~/.dsh-desktop/bin/dsh.cmd`（Windows）；**不会**自动改 shell rc
- 重启 dsh、npm 源、应用更新
- 应用菜单：重启 dsh / 桌面设置 / 插件向导

dsh 意外退出后会自动重启（最多 3 次，带退避）；主动退出或手动重启不会触发。

## 技术说明

- UI 启动页：Vite + TypeScript（连接中 / 失败重试）
- Rust：`HarnessManager` 负责启动子进程、解析就绪 URL、`navigate` 到本机 Web UI
- 优先使用全局 `dsh`；否则 `npx --yes @deepseek-ai/dsh web --port <port>`

## License

MIT
