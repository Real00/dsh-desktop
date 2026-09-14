# DSH Desktop

开箱即用的 [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)（`dsh`）桌面壳，基于 **Tauri 2**。

打开应用后会自动：

1. 检测本机 `dsh` 或 `npx`
2. 启动 `dsh web`（随机本机端口）
3. 等待就绪后在窗口内打开官方 Web UI
4. 退出时结束子进程

> 本项目**不重写** DSH Web UI，只做原生窗口托管与进程管理。

## 前置条件

- macOS 11+ / Windows / Linux（开发机）
- [Node.js](https://nodejs.org/) **≥ 22.19**（运行时用来拉起 `@deepseek-ai/dsh`）
- 开发还需要：Rust stable、系统 WebView 依赖（见 [Tauri prerequisites](https://tauri.app/start/prerequisites/)）

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

本地打 macOS 包：

```bash
npm run tauri build
# 产物通常在 src-tauri/target/release/bundle/{dmg,macos}
```

## GitHub Actions（macOS 包）

工作流文件：

| 文件 | 作用 |
| --- | --- |
| `.github/workflows/ci.yml` | PR / push 时构建前端 |
| `.github/workflows/release-macos.yml` | 打 **Apple Silicon + Intel** macOS 包 |

触发方式：

- 推送标签：`git tag v0.1.0 && git push origin v0.1.0`
- 或在 Actions 里手动 `workflow_dispatch`

产物通过 `tauri-apps/tauri-action` 发到 **Draft Release**（`dmg` / `app`）。

### 代码签名（可选）

默认不做 Apple Developer 签名，本机打开可能需要「右键 → 打开」。若要签名 / 公证，在仓库 Secrets 中配置后取消 workflow 里相关环境变量注释：

- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`

## 技术说明

- UI 启动页：Vite + TypeScript（连接中 / 失败重试）
- Rust：`HarnessManager` 负责启动子进程、解析就绪 URL、`navigate` 到本机 Web UI
- 优先使用全局 `dsh`；否则 `npx --yes @deepseek-ai/dsh web --port <port>`

## License

MIT
