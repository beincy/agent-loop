import { loadConfig, AGENT_LOOP_DIR } from './config.js';
import { Subscriber } from './subscriber.js';
import { initializeAllReporters, disposeAllReporters } from './reporters/index.js';
import type { SubscriptionConfig } from './types.js';

export interface AppOptions {
  /**
   * 快速模式：直接传入一个订阅配置，不读取配置文件。
   * 由 CLI 的 --url / --prompt 参数构建后传入。
   */
  quickSubscription?: SubscriptionConfig;
}

/**
 * 应用主逻辑
 */
export async function runApp(options: AppOptions = {}): Promise<void> {
  console.log('🔁 Agent Loop 启动中...\n');

  await initializeAllReporters();

  let subscriptions: SubscriptionConfig[];
  let defaultWorkspace: string;

  if (options.quickSubscription) {
    // ── 快速模式 ────────────────────────────────────────────
    subscriptions = [options.quickSubscription];
    defaultWorkspace = options.quickSubscription.workspace ?? process.cwd();
    console.log(`⚡ 快速模式 | URL: ${options.quickSubscription.smeeUrl}`);
  } else {
    // ── 配置文件模式 ────────────────────────────────────────
    const config = loadConfig();
    subscriptions = config.subscriptions.filter((s) => s.enabled);
    defaultWorkspace = config.defaultWorkspace;

    console.log(`📁 配置目录: ${AGENT_LOOP_DIR}`);
    console.log(`📁 默认工作区: ${defaultWorkspace}`);

    if (subscriptions.length === 0) {
      console.warn(
        '⚠️  没有已启用的订阅。\n' +
        `   请编辑 ${AGENT_LOOP_DIR}/config.json，添加订阅并将 enabled 设为 true。`
      );
      await disposeAllReporters();
      return;
    }
  }

  const concurrent = process.env.LOOP_CONCURRENT?.toLowerCase() === 'true';
  console.log(`⚙️  执行模式: ${concurrent ? '并发' : '串行（默认）'}`);
  console.log(`📋 活跃订阅数: ${subscriptions.length}\n`);

  // 启动所有订阅
  const subscribers: Subscriber[] = [];
  for (const sub of subscriptions) {
    const subscriber = new Subscriber(sub, defaultWorkspace);
    await subscriber.start();
    subscribers.push(subscriber);
  }

  console.log('\n✅ 所有订阅已启动，等待事件中...');
  console.log('按 Ctrl+C 退出\n');

  // 优雅退出
  const shutdown = async (signal: string) => {
    console.log(`\n\n🛑 收到 ${signal} 信号，正在关闭...`);
    for (const sub of subscribers) {
      sub.stop();
    }
    await disposeAllReporters();
    console.log('👋 Agent Loop 已退出');
    process.exit(0);
  };

  process.on('SIGINT', () => void shutdown('SIGINT'));
  process.on('SIGTERM', () => void shutdown('SIGTERM'));
}
