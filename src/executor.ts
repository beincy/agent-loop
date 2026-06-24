import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import type { SubscriptionConfig, SmeeEventData, ExecutionResult } from './types.js';

const execFileAsync = promisify(execFile);

/**
 * 将 Smee 事件数据和基础提示词合并成完整的提示词
 */
function buildPrompt(basePrompt: string, event: SmeeEventData): string {
  const bodyStr =
    typeof event.body === 'string'
      ? event.body
      : JSON.stringify(event.body, null, 2);

  const headersStr = JSON.stringify(event.headers, null, 2);
  const queryStr =
    Object.keys(event.query).length > 0
      ? JSON.stringify(event.query, null, 2)
      : '（无）';

  return [
    basePrompt.trim(),
    '',
    '---',
    '以下是通过 Webhook 接收到的事件信息，请结合上述指令处理：',
    '',
    '## 请求头（Headers）',
    '```json',
    headersStr,
    '```',
    '',
    '## 查询参数（Query Parameters）',
    '```json',
    queryStr,
    '```',
    '',
    '## 请求体（Body）',
    '```',
    bodyStr,
    '```',
    '',
    `> 事件接收时间：${new Date(event.timestamp).toISOString()}`,
  ].join('\n');
}

/**
 * 调用 Claude CLI 执行提示词
 * 使用 claude -p "<prompt>" --dangerously-skip-permissions
 */
async function runClaude(
  prompt: string,
  workspace: string
): Promise<{ output: string; success: boolean; error?: string }> {
  try {
    const { stdout, stderr } = await execFileAsync(
      'claude',
      ['-p', prompt, '--dangerously-skip-permissions'],
      {
        cwd: workspace,
        // 不设置超时，让 claude 自行完成
        maxBuffer: 100 * 1024 * 1024, // 100MB 输出缓冲
      }
    );

    const output = stdout + (stderr ? `\n[stderr]\n${stderr}` : '');
    return { output, success: true };
  } catch (err: unknown) {
    const error = err as NodeJS.ErrnoException & {
      stdout?: string;
      stderr?: string;
    };
    return {
      output: [error.stdout ?? '', error.stderr ?? ''].filter(Boolean).join('\n'),
      success: false,
      error: error.message,
    };
  }
}

/**
 * 执行器主函数：将事件组装为提示词并调用 Claude
 */
export async function executeWithClaude(
  subscription: SubscriptionConfig,
  defaultWorkspace: string,
  event: SmeeEventData
): Promise<ExecutionResult> {
  const workspace = subscription.workspace ?? defaultWorkspace;
  const prompt = buildPrompt(subscription.basePrompt, event);
  const startedAt = new Date();

  console.log(
    `\n🤖 [${subscription.name}] 开始调用 Claude（工作区: ${workspace}）...`
  );

  const { output, success, error } = await runClaude(prompt, workspace);
  const finishedAt = new Date();

  return {
    subscriptionName: subscription.name,
    success,
    output,
    error,
    startedAt,
    finishedAt,
    prompt,
  };
}
