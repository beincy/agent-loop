

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
将自定义 filter WASM 文件放在 `~/.agent-loop/policy/` 目录：
```bash
~/.agent-loop/
  └── policy/
      ├── issue-mention.wasm           # issue/评论中提及 @claude0805 才放行
      ├── issue-mention-claude01.wasm  # issue/评论中提及 @claude01 才放行
      ├── pr-mention.wasm              # 新建 PR 标题/正文提及 @claude0805 才放行
      └── security-filter.wasm
```

插件会自动在 GUI 的下拉框中显示。

### 构建 WASM 插件
参考 `wasm-plugins/` 目录下的示例：
- `console-reporter`: Reporter 插件示例
- `issue-mention` / `issue-mention-claude01` / `pr-mention` / `security-filter`: Filter 插件示例

构建并安装（以 `issue-mention-claude01` 为例，需先安装 wasm 目标：`rustup target add wasm32-unknown-unknown`）：

```bash
# 1. 编译为 wasm（在插件目录内执行）
cd wasm-plugins/issue-mention-claude01
cargo build --target wasm32-unknown-unknown --release

# 2. 拷贝到插件目录（注意：cargo 产物文件名是下划线，拷贝时改成连字符命名）
#    Filter 插件 → ~/.agent-loop/policy/
cp target/wasm32-unknown-unknown/release/issue_mention_claude01.wasm \
   ~/.agent-loop/policy/issue-mention-claude01.wasm

#    Reporter 插件 → ~/.agent-loop/reporters/
# cp target/wasm32-unknown-unknown/release/console_reporter.wasm \
#    ~/.agent-loop/reporters/console-reporter.wasm
```

其他插件同理，例如重新编译安装 `issue-mention`：

```bash
cd wasm-plugins/issue-mention
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/issue_mention.wasm \
   ~/.agent-loop/policy/issue-mention.wasm
```

安装后重启 GUI，即可在 Filter 下拉框中选择新插件（配置中 `wasmPolicy` 的值为文件名去掉 `.wasm` 后缀，如 `issue-mention-claude01`）。
