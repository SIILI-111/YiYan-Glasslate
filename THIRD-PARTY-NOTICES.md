# 第三方依赖许可

本项目以 MIT 发布（见 [LICENSE](LICENSE)）。以下为直接依赖的许可证情况。

> 说明：本表只列**直接依赖**。完整的传递依赖清单应在每次发版前用 `cargo about` / `cargo deny check licenses` / `npx license-checker` 重新生成并随包分发。

## Rust 侧

| 依赖 | 用途 | 许可证 |
| --- | --- | --- |
| `tauri` / `tauri-build` / `tauri-plugin-opener` | 应用外壳、窗口、插件 | MIT OR Apache-2.0 |
| `serde` / `serde_json` | 配置与 IPC 序列化 | MIT OR Apache-2.0 |
| `base64` | 图片与凭据编码 | MIT OR Apache-2.0 |
| `image` | PNG 编码（M0 抓屏落盘） | MIT OR Apache-2.0 |
| `windows`（M1 计划引入） | WinRT OCR / Win32 调用 | MIT OR Apache-2.0 |
| `xcap`（待评估，M2） | 跨平台截屏，含 Windows.Graphics.Capture | Apache-2.0 |
| `ort` / ONNX Runtime（M4 计划引入） | 离线 OCR 推理 | MIT（ONNX Runtime 本体 MIT） |

## 前端侧

| 依赖 | 用途 | 许可证 |
| --- | --- | --- |
| `vite` | 构建 | MIT |
| `typescript` | 类型检查 | Apache-2.0 |
| `@tauri-apps/api` / `@tauri-apps/cli` | 前端 API 与构建工具 | MIT OR Apache-2.0 |

## 运行期外部组件（不随包分发）

| 组件 | 用途 | 授权 |
| --- | --- | --- |
| `Windows.Media.OCR` | 系统 OCR 引擎 | Windows 系统 API，随系统授权 |
| WebView2 Runtime | 前端渲染 | 随 Windows / Edge 分发 |
| 阿里云机器翻译 API | 翻译 | 用户自带 AccessKey，按服务商条款计费 |
| PP-OCRv6 small ONNX 模型（M4） | 离线 OCR 模型 | PaddleOCR / RapidOCR 项目，Apache-2.0 |

## 已否决的依赖

| 组件 | 原因 |
| --- | --- |
| Pot（`pot-desktop`） | GPL-3.0，衍生作品必须同样开源，与 MIT 冲突 |
| Tesseract | 体积与中文/英文混合识别质量均不如目标方案，暂不需要 |
| 任何本地大模型推理 | 体积与内存成本与"轻量"定位冲突 |
