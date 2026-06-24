import { createServer as createHttpServer } from 'node:http';
import { createServer as createNetServer } from 'node:net';
import type { IncomingMessage, ServerResponse, Server } from 'node:http';
import SmeeClient from 'smee-client';
import type { SubscriptionConfig, SmeeEventData } from './types.js';
import { executeWithClaude } from './executor.js';
import { getReporter } from './reporters/index.js';

/** smee-client 的 start() 返回值类型 */
interface SmeeEvents {
  close: () => void;
}

/**
 * 判断是否开启并发执行
 * 环境变量 LOOP_CONCURRENT=true 时并发，否则串行（默认串行）
 */
function isConcurrent(): boolean {
  return process.env.LOOP_CONCURRENT?.toLowerCase() === 'true';
}

/**
 * 获取一个随机可用端口
 */
function getAvailablePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createNetServer();
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address();
      srv.close(() => {
        if (addr && typeof addr === 'object') {
          resolve(addr.port);
        } else {
          reject(new Error('无法获取可用端口'));
        }
      });
    });
  });
}

/**
 * 从 smee-client 转发的原始 body 中提取结构化事件数据
 */
function extractEventData(
  reqHeaders: Record<string, string | string[] | undefined>,
  body: Record<string, unknown>
): SmeeEventData {
  // smee-client 在转发时可能把原始 headers 放在 body.headers 中
  let headers: Record<string, string> = {};
  if (body.headers && typeof body.headers === 'object') {
    headers = body.headers as Record<string, string>;
  } else {
    const skipHeaders = new Set(['host', 'connection', 'transfer-encoding', 'content-length']);
    for (const [k, v] of Object.entries(reqHeaders)) {
      if (!skipHeaders.has(k) && v !== undefined) {
        headers[k] = Array.isArray(v) ? v.join(', ') : v;
      }
    }
  }

  const query =
    body.query && typeof body.query === 'object'
      ? (body.query as Record<string, string>)
      : {};

  // body 字段优先，否则把整个 payload 当作 body
  const eventBody =
    'body' in body ? body.body : (({ headers: _h, query: _q, ...rest }) => rest)(body);

  return {
    headers,
    query,
    body: eventBody,
    timestamp: Date.now(),
  };
}

/**
 * 单个订阅的运行时实例
 */
export class Subscriber {
  private server: Server | null = null;
  private smeeEvents: SmeeEvents | null = null;
  /** 串行执行队列（并发模式下不使用） */
  private executionQueue = Promise.resolve();

  constructor(
    private readonly subscription: SubscriptionConfig,
    private readonly defaultWorkspace: string
  ) {}

  /**
   * 启动订阅：创建本地 HTTP 服务 + 注册 smee-client 转发
   */
  async start(): Promise<void> {
    const port = await getAvailablePort();
    const targetUrl = `http://127.0.0.1:${port}/events`;

    this.server = createHttpServer(this.handleRequest.bind(this));

    await new Promise<void>((resolve) => {
      this.server!.listen(port, '127.0.0.1', resolve);
    });

    console.log(
      `📡 [${this.subscription.name}] 已订阅 ${this.subscription.smeeUrl}`
    );
    console.log(`   → 本地监听 ${targetUrl}`);

    const smee = new SmeeClient({
      source: this.subscription.smeeUrl,
      target: targetUrl,
      logger: {
        info: (msg: string) =>
          console.log(`   ℹ️  [${this.subscription.name}] ${msg}`),
        error: (msg: string) =>
          console.error(`   ❌ [${this.subscription.name}] ${msg}`),
      },
    });

    this.smeeEvents = smee.start() as SmeeEvents;
  }

  /**
   * 停止订阅
   */
  stop(): void {
    this.smeeEvents?.close();
    this.server?.close();
  }

  /**
   * 处理 smee-client 转发过来的 HTTP 请求
   */
  private handleRequest(req: IncomingMessage, res: ServerResponse): void {
    if (req.method !== 'POST') {
      res.writeHead(405).end();
      return;
    }

    let rawData = '';
    req.on('data', (chunk: Buffer) => {
      rawData += chunk.toString();
    });

    req.on('end', () => {
      try {
        const parsed = JSON.parse(rawData) as Record<string, unknown>;
        const eventData = extractEventData(
          req.headers as Record<string, string | string[] | undefined>,
          parsed
        );
        this.scheduleExecution(eventData);
      } catch (err) {
        console.error(
          `[${this.subscription.name}] 解析事件数据失败:`,
          (err as Error).message
        );
      }
      res.writeHead(200).end('OK');
    });
  }

  /**
   * 根据并发/串行配置调度 Claude 执行任务
   */
  private scheduleExecution(eventData: SmeeEventData): void {
    const reporter = getReporter(this.subscription.reporter);

    const executeTask = async () => {
      console.log(
        `\n📨 [${this.subscription.name}] 收到事件，准备调用 Claude...`
      );
      const result = await executeWithClaude(
        this.subscription,
        this.defaultWorkspace,
        eventData
      );
      await reporter.report(result);
    };

    if (isConcurrent()) {
      // 并发：直接启动
      executeTask().catch((err) => {
        console.error(
          `[${this.subscription.name}] 执行异常:`,
          (err as Error).message
        );
      });
    } else {
      // 串行：加入队列
      this.executionQueue = this.executionQueue
        .then(executeTask)
        .catch((err) => {
          console.error(
            `[${this.subscription.name}] 执行异常:`,
            (err as Error).message
          );
        });
    }
  }
}
