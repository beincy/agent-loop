#!/usr/bin/env node
import 'dotenv/config';
import { program } from 'commander';
import { createRequire } from 'node:module';
import { runApp } from './index.js';
import type { SubscriptionConfig } from './types.js';

// 从 package.json 读取版本号
const require = createRequire(import.meta.url);
// eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
const pkg = require('../package.json') as { version: string; description: string };

program
  .name('agent-loop')
  .description(pkg.description)
  .version(pkg.version, '-v, --version')
  // ── 快速模式参数 ───────────────────────────────────────────
  .option('-u, --url <url>', 'Smee 频道 URL（快速模式，不读取配置文件）')
  .option('-p, --prompt <text>', '基础提示词（快速模式）')
  .option('-r, --reporter <name>', '汇报器名称', 'console')
  .option('-w, --workspace <path>', '工作区目录（快速模式，默认当前目录）')
  // ── 全局选项 ───────────────────────────────────────────────
  .option('-c, --config <path>', '配置文件路径（覆盖默认 ~/.agent-loop/config.json）')
  .option('--concurrent', '启用并发执行（默认串行）')
  .addHelpText(
    'after',
    `
示例:
  # 配置文件模式（读取 ~/.agent-loop/config.json）
  $ agent-loop

  # 快速模式（直接指定 URL 和提示词）
  $ agent-loop --url "https://smee.io/YOUR_ID" --prompt "请处理这个事件"

  # 快速模式 + 指定工作区 + 并发执行
  $ agent-loop -u "https://smee.io/YOUR_ID" -p "处理事件" -w ~/my-project --concurrent

  # 使用自定义配置文件
  $ agent-loop --config ./my-config.json
`
  )
  .parse();

const opts = program.opts<{
  url?: string;
  prompt?: string;
  reporter: string;
  workspace?: string;
  config?: string;
  concurrent?: boolean;
}>();

// 将 CLI flag 同步到环境变量，供内部模块读取
if (opts.concurrent) {
  process.env.LOOP_CONCURRENT = 'true';
}
if (opts.config) {
  process.env.LOOP_CONFIG = opts.config;
}

try {
  if (opts.url) {
    // ── 快速模式 ──────────────────────────────────────────────
    if (!opts.prompt) {
      console.error('❌ 快速模式下必须提供 -p/--prompt 参数');
      process.exit(1);
    }

    const quickSub: SubscriptionConfig = {
      name: 'quick',
      smeeUrl: opts.url,
      enabled: true,
      basePrompt: opts.prompt,
      workspace: opts.workspace ?? process.cwd(),
      reporter: opts.reporter,
    };

    await runApp({ quickSubscription: quickSub });
  } else {
    // ── 配置文件模式 ──────────────────────────────────────────
    await runApp();
  }
} catch (err) {
  console.error('❌ 启动失败:', (err as Error).message);
  process.exit(1);
}
