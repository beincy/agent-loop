

### 模块结构
- `gui_app.rs`: GUI 主界面和交互逻辑
- `gui_config.rs`: 配置数据结构和持久化
- `ws_client.rs`: WebSocket 客户端实现
- `main.rs`: 应用入口点
- 其他模块: executor, filter, reporter, subscriber (保留自原版)

### WebSocket 协议
参考 `cc-connect-ws手册说明.md` 了解 cc-connect WebSocket API。

## 对比旧版

| 特性 | 旧版 (Node.js) | 新版 (Rust GUI) |
|------|---------------|----------------|
| 语言 | TypeScript | Rust |
| 界面 | CLI | GUI (egui) |
| Claude 调用 | 命令行 | WebSocket |
| 配置 | JSON 文件 | GUI + JSON |
| 依赖 | Node.js, npm | 仅 Rust |
| 启动方式 | `npm start` | 双击运行 |

## 开发

### 调试模式
```bash
cargo run
```

### 发布构建
```bash
cargo build --release
# 可执行文件位于: target/release/agent-loop
```

## 故障排除

### GUI 无法启动
确保系统支持图形界面渲染库 (OpenGL/Metal/DirectX)

### WebSocket 连接失败
- 检查 cc-connect 服务是否运行
- 验证 WebSocket URL 是否正确
- 查看控制台错误信息

### 订阅无反应
- 确认 Smee URL 是否有效
- 检查工作区路径是否存在
- 查看状态栏提示信息

## License

MIT

## WASM 插件扩展

### Reporter 插件
将自定义 reporter WASM 文件放在 `~/.agent-loop/reporters/` 目录：
```bash
~/.agent-loop/
  └── reporters/
      ├── console.wasm
      └── custom-reporter.wasm
```

### Filter 插件
将自定义 filter WASM 文件放在 `~/.agent-loop/filters/` 目录：
```bash
~/.agent-loop/
  └── filters/
      ├── security-filter.wasm
      └── custom-filter.wasm
```

插件会自动在 GUI 的下拉框中显示。

### 构建 WASM 插件
参考 `wasm-plugins/` 目录下的示例：
- `console-reporter`: Reporter 插件示例
- `security-filter`: Filter 插件示例

构建步骤：
```bash
cd wasm-plugins/your-plugin
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/your_plugin.wasm ~/.agent-loop/reporters/
# 或
cp target/wasm32-unknown-unknown/release/your_plugin.wasm ~/.agent-loop/filters/
```
