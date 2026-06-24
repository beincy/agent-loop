import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { LoopConfig } from './types.js';

/** ~/.agent-loop 目录路径（多平台兼容） */
export const AGENT_LOOP_DIR = path.join(os.homedir(), '.agent-loop');

/** 默认配置文件路径 */
const DEFAULT_CONFIG_PATH = path.join(AGENT_LOOP_DIR, 'config.json');

/** 默认配置内容（空订阅列表） */
const DEFAULT_CONFIG: LoopConfig = {
  defaultWorkspace: os.homedir(),
  subscriptions: [],
};

/**
 * 确保 ~/.agent-loop 目录和默认配置文件存在。
 * 若目录或文件不存在则自动创建，不会覆盖已有配置。
 */
function ensureConfigDir(): void {
  if (!fs.existsSync(AGENT_LOOP_DIR)) {
    fs.mkdirSync(AGENT_LOOP_DIR, { recursive: true });
    console.log(`📁 已创建配置目录: ${AGENT_LOOP_DIR}`);
  }

  if (!fs.existsSync(DEFAULT_CONFIG_PATH)) {
    fs.writeFileSync(
      DEFAULT_CONFIG_PATH,
      JSON.stringify(DEFAULT_CONFIG, null, 2) + '\n',
      'utf-8'
    );
    console.log(`📝 已生成默认配置: ${DEFAULT_CONFIG_PATH}`);
  }
}

/**
 * 加载并验证配置文件。
 *
 * 优先级：
 *   1. 环境变量 LOOP_CONFIG 指定的路径
 *   2. ~/.agent-loop/config.json（不存在时自动创建）
 */
export function loadConfig(): LoopConfig {
  // 若未通过环境变量指定，先确保默认路径和文件存在
  const configPath = process.env.LOOP_CONFIG
    ? path.resolve(process.env.LOOP_CONFIG)
    : DEFAULT_CONFIG_PATH;

  if (!process.env.LOOP_CONFIG) {
    ensureConfigDir();
  }

  if (!fs.existsSync(configPath)) {
    throw new Error(
      `配置文件不存在: ${configPath}\n` +
      `请检查 LOOP_CONFIG 环境变量或 ${DEFAULT_CONFIG_PATH}`
    );
  }

  let raw: unknown;
  try {
    raw = JSON.parse(fs.readFileSync(configPath, 'utf-8'));
  } catch (err) {
    throw new Error(`解析配置文件失败 (${configPath}): ${(err as Error).message}`);
  }

  validateConfig(raw);
  return raw as LoopConfig;
}

/**
 * 简单校验配置结构
 */
function validateConfig(raw: unknown): asserts raw is LoopConfig {
  if (typeof raw !== 'object' || raw === null) {
    throw new Error('配置文件格式错误：根节点必须是一个 JSON 对象');
  }

  const config = raw as Record<string, unknown>;

  if (typeof config.defaultWorkspace !== 'string') {
    throw new Error('配置文件格式错误：缺少 defaultWorkspace 字段（字符串）');
  }

  if (!Array.isArray(config.subscriptions)) {
    throw new Error('配置文件格式错误：subscriptions 必须是一个数组');
  }

  for (const [i, sub] of config.subscriptions.entries()) {
    if (typeof sub !== 'object' || sub === null) {
      throw new Error(`subscriptions[${i}] 必须是一个对象`);
    }
    const s = sub as Record<string, unknown>;
    if (typeof s.name !== 'string' || !s.name) {
      throw new Error(`subscriptions[${i}].name 必须是非空字符串`);
    }
    if (typeof s.smeeUrl !== 'string' || !s.smeeUrl) {
      throw new Error(`subscriptions[${i}].smeeUrl 必须是非空字符串`);
    }
    if (typeof s.enabled !== 'boolean') {
      throw new Error(`subscriptions[${i}].enabled 必须是布尔值`);
    }
    if (typeof s.basePrompt !== 'string') {
      throw new Error(`subscriptions[${i}].basePrompt 必须是字符串`);
    }
  }
}
