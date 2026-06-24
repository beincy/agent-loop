import fs from 'node:fs';
import path from 'node:path';
import type { Reporter } from './base.js';
import type { ExecutionResult } from '../types.js';

/**
 * 默认汇报器：将执行结果打印到控制台，同时写入日志文件。
 *
 * 日志目录由环境变量 LOOP_LOG_DIR 指定，默认为 ./logs。
 * 每条订阅对应一个独立的日志文件，文件名为 <subscriptionName>.log。
 */
export class ConsoleReporter implements Reporter {
  readonly name = 'console';

  private logDir: string;

  constructor() {
    this.logDir = path.resolve(process.env.LOOP_LOG_DIR ?? './logs');
  }

  async initialize(): Promise<void> {
    fs.mkdirSync(this.logDir, { recursive: true });
  }

  async report(result: ExecutionResult): Promise<void> {
    const separator = '─'.repeat(60);
    const statusIcon = result.success ? '✅' : '❌';
    const duration = result.finishedAt.getTime() - result.startedAt.getTime();

    // ── 控制台输出 ──────────────────────────────────────────
    console.log(`\n${separator}`);
    console.log(`${statusIcon} [${result.subscriptionName}] 执行完成`);
    console.log(`   开始: ${result.startedAt.toISOString()}`);
    console.log(`   耗时: ${duration}ms`);
    console.log(separator);

    if (result.success) {
      console.log(result.output);
    } else {
      console.error(`❌ 错误: ${result.error}`);
      if (result.output) {
        console.log(`输出:\n${result.output}`);
      }
    }
    console.log(`${separator}\n`);

    // ── 写入日志文件 ──────────────────────────────────────────
    const logFile = path.join(this.logDir, `${result.subscriptionName}.log`);
    const logEntry = this.formatLogEntry(result, duration);
    fs.appendFileSync(logFile, logEntry, 'utf-8');
  }

  private formatLogEntry(result: ExecutionResult, durationMs: number): string {
    const divider = '='.repeat(80);
    const lines: string[] = [
      divider,
      `[${result.startedAt.toISOString()}] 订阅: ${result.subscriptionName}`,
      `状态: ${result.success ? 'SUCCESS' : 'FAILURE'} | 耗时: ${durationMs}ms`,
      `完成时间: ${result.finishedAt.toISOString()}`,
      '',
      '── 提示词 ──',
      result.prompt,
      '',
      '── 输出 ──',
      result.success ? result.output : `错误: ${result.error}`,
    ];

    if (!result.success && result.output) {
      lines.push('', '── 部分输出 ──', result.output);
    }

    lines.push(divider, '');
    return lines.join('\n');
  }
}
