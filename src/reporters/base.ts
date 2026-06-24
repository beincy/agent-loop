import type { ExecutionResult } from '../types.js';

/**
 * 汇报器接口（可插拔设计）
 * 所有自定义汇报器必须实现此接口。
 *
 * 使用方式：
 *   1. 实现此接口，创建一个新的汇报器类
 *   2. 在 reporters/index.ts 中注册
 *   3. 在 config.json 的 reporter 字段中使用对应名称
 */
export interface Reporter {
  /** 汇报器唯一名称，对应 config.json 中 reporter 字段 */
  readonly name: string;

  /**
   * 汇报执行结果
   * @param result Claude 执行结果
   */
  report(result: ExecutionResult): Promise<void>;

  /**
   * 可选：汇报器启动时的初始化逻辑（如建立连接、创建目录等）
   */
  initialize?(): Promise<void>;

  /**
   * 可选：程序退出时的清理逻辑（如关闭连接、刷新缓冲区等）
   */
  dispose?(): Promise<void>;
}
