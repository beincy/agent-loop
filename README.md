# Agent Loop 🔁

`agent-loop` 是一个用于本地执行的 CLI 工具，专门负责订阅远程 Webhook 事件（通过 [smee.io](https://smee.io/)），并在收到事件时，将“基础提示词”和“事件详情（Headers, Body, Query）”合并，调用本地 `claude` 命令行工具（Claude Code CLI）来执行自动化代码操作。

---

## 🚀 功能特点

1. **多平台 CLI 支持**：可作为 npm 包全局安装，在终端通过 `agent-loop` 命令直接运行。
2. **免配置快速启动**：支持使用 `-u` (url) 和 `-p` (prompt) 命令行参数直接启动监听，无需编写任何配置文件。
3. **配置文件管理**：支持多订阅并行监听。首次运行时，会自动在用户家目录创建 `~/.agent-loop/config.json` 默认配置文件。
4. **灵活的并发控制**：默认对接收到的事件进行串行排队执行（防止本地 `claude` 实例冲突），也可通过 `--concurrent` 参数或环境变量开启并发模式。
5. **可插拔的汇报器（Reporter）**：内置 `console` 汇报器，支持在终端打印执行结果并追加至本地日志文件。可根据需求轻松扩展其他汇报通道（如 Webhook / 飞书通知等）。

---

## 📦 安装

首先确保本地已全局安装并配置好了 `claude` 命令行工具。

然后在本项目路径下进行全局链接，或直接通过 npm 安装：

```bash
# 本地链接开发版本
pnpm link --global

# 或者从 npm 安装（发布后）
npm install -g agent-loop
```

---

## 💻 使用方法

`agent-loop` 支持两种工作模式：**快速模式** 和 **配置文件模式**。

### 1. 快速模式（免配置文件）
直接在命令行指定监听 Jun URL 和基础提示词，适合临时测试或单任务自动化。

```bash
# 基本用法
agent-loop -u "https://smee.io/YOUR_CHANNEL_ID" -p "你是一个自动化助手。请根据以下 Webhook 变动，修改并实现项目里的 TODO 任务。"

# 显式指定工作区目录（默认为当前执行命令的目录）
agent-loop -u "https://smee.io/YOUR_CHANNEL_ID" -p "分析事件" -w /Users/username/my-project

# 开启并发执行
agent-loop -u "https://smee.io/YOUR_CHANNEL_ID" -p "分析事件" --concurrent
```

### 2. 配置文件模式
如果不带 `-u` 参数直接运行，程序将读取配置文件。

```bash
agent-loop
```

#### 配置路径与默认配置
首次运行 `agent-loop` 时，程序会自动在当前用户 Home 目录下生成配置目录和默认配置文件：
- 目录：`~/.agent-loop/`
- 配置文件：`~/.agent-loop/config.json`

**默认生成的 `config.json` 格式如下：**
```json
{
  "defaultWorkspace": "/Users/your-username",
  "subscriptions": []
}
```

#### 配置多订阅示例
你可以编辑 `~/.agent-loop/config.json` 并添加多个订阅：

```json
{
  "defaultWorkspace": "/Users/username/Documents/Code",
  "subscriptions": [
    {
      "name": "auto-coder",
      "smeeUrl": "https://smee.io/xiKHIyoQtCslLWF",
      "enabled": true,
      "basePrompt": "请根据事件修改代码，然后运行测试确保通过。",
      "workspace": "/Users/username/Documents/Code/my-app",
      "reporter": "console"
    },
    {
      "name": "ci-helper",
      "smeeUrl": "https://smee.io/another-smee-channel",
      "enabled": false,
      "basePrompt": "分析此事件并输出总结。",
      "reporter": "console"
    }
  ]
}
```

> **注意：**
> - `workspace` 字段为选填，若不填写，则使用外部的 `defaultWorkspace`。
> - 你可以通过将 `enabled` 设为 `false` 来临时禁用某条订阅。

---

## ⚙️ 命令行参数与环境变量

### 命令行参数

可以使用 `agent-loop --help` 查看所有可用选项：

- `-v, --version`：输出当前版本号。
- `-u, --url <url>`：Smee 频道 URL（启用快速模式）。
- `-p, --prompt <text>`：接收事件时的基础指令提示词（快速模式必填）。
- `-r, --reporter <name>`：指定执行结果汇报器（默认: `console`）。
- `-w, --workspace <path>`：指定执行 Claude 时的根目录工作区（快速模式下默认当前目录）。
- `-c, --config <path>`：手动指定配置文件路径，覆盖默认的 `~/.agent-loop/config.json`。
- `--concurrent`：启用并发执行（多个事件同时调用多个 Claude，默认是串行排队）。

### 环境变量

你可以通过本地 `.env` 文件或系统环境变量来控制部分全局表现：

* `LOOP_CONCURRENT=true`：启用并发执行模式（等同于 `--concurrent`）。
* `LOOP_CONFIG=/path/to/config.json`：手动指定配置文件路径（等同于 `-c`）。
* `LOOP_LOG_DIR=/path/to/logs`：重定向 `console` 汇报器的日志输出目录（默认输出在 `./logs`）。

---

## 🛠️ 进阶开发：自定义汇报器 (Reporter)

项目采用了可插拔的设计。如果以后想添加如“钉钉通知”、“飞书 Webhook”等汇报器：

1. 在 `src/reporters/` 目录下新建汇报器文件并实现 `src/reporters/base.ts` 中的 `Reporter` 接口：
   ```typescript
   import { Reporter } from './base.js';
   import type { ExecutionResult } from '../types.js';

   export class CustomReporter implements Reporter {
     readonly name = 'my-custom-reporter';

     async report(result: ExecutionResult): Promise<void> {
       // 实现发送逻辑，例如 fetch 发送 Webhook
     }
   }
   ```
2. 在 [src/reporters/index.ts](file:///Users/bianhui/Documents/Code/loop/src/reporters/index.ts) 中注册该类：
   ```typescript
   registry.set('my-custom-reporter', new CustomReporter());
   ```
3. 在 `config.json` 对应的订阅项中，将 `"reporter": "console"` 改为 `"reporter": "my-custom-reporter"`。
