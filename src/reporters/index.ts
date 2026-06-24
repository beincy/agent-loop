import type { Reporter } from './base.js';
import { ConsoleReporter } from './console-reporter.js';

/**
 * 汇报器注册表
 *
 * 新增自定义汇报器步骤：
 *   1. 实现 Reporter 接口（src/reporters/base.ts）
 *   2. 在下方 registry 中注册，key 为汇报器名称
 *   3. 在 config.json 的对应订阅中填写该名称
 */
const registry: Map<string, Reporter> = new Map();

// ── 注册内置汇报器 ──────────────────────────────────────────
registry.set('console', new ConsoleReporter());

/**
 * 根据名称获取汇报器，不存在时回退到 console 汇报器
 */
export function getReporter(name?: string): Reporter {
  const key = name ?? 'console';
  const reporter = registry.get(key);
  if (!reporter) {
    console.warn(`⚠️  未找到汇报器 "${key}"，回退到 "console" 汇报器`);
    return registry.get('console')!;
  }
  return reporter;
}

/**
 * 初始化所有已注册的汇报器
 */
export async function initializeAllReporters(): Promise<void> {
  for (const reporter of registry.values()) {
    await reporter.initialize?.();
  }
}

/**
 * 清理所有已注册的汇报器
 */
export async function disposeAllReporters(): Promise<void> {
  for (const reporter of registry.values()) {
    await reporter.dispose?.();
  }
}
