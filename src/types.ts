// ================================
// 配置类型定义
// ================================

/** 单条订阅配置 */
export interface SubscriptionConfig {
  /** 订阅名称（唯一标识） */
  name: string;
  /** Smee.io 频道 URL */
  smeeUrl: string;
  /** 是否启用 */
  enabled: boolean;
  /** 基础提示词，会与收到的事件内容合并后发给 Claude */
  basePrompt: string;
  /** 工作区目录，不填则使用 defaultWorkspace */
  workspace?: string;
  /** 汇报器名称，不填则使用 "console" */
  reporter?: string;
}

/** 根配置 */
export interface LoopConfig {
  /** 默认工作区目录 */
  defaultWorkspace: string;
  /** 订阅列表 */
  subscriptions: SubscriptionConfig[];
}

// ================================
// 事件类型定义
// ================================

/** Smee 转发过来的原始事件数据 */
export interface SmeeEventData {
  /** 请求头 */
  headers: Record<string, string>;
  /** 查询参数 */
  query: Record<string, string>;
  /** 请求体（原始字符串或已解析的对象） */
  body: unknown;
  /** 时间戳 */
  timestamp: number;
}

// ================================
// 执行结果类型
// ================================

/** Claude 执行结果 */
export interface ExecutionResult {
  /** 所属订阅名称 */
  subscriptionName: string;
  /** 执行是否成功 */
  success: boolean;
  /** Claude 输出内容 */
  output: string;
  /** 错误信息（失败时） */
  error?: string;
  /** 执行开始时间 */
  startedAt: Date;
  /** 执行结束时间 */
  finishedAt: Date;
  /** 发送给 Claude 的完整提示词 */
  prompt: string;
}
