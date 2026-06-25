mod config;
mod executor;
mod reporter;
mod subscriber;
mod types;

use anyhow::Result;
use clap::Parser;
use reporter::get_reporter;
use tokio::signal;
use types::SubscriptionConfig;

/// Loop 本地 Claude Code CLI 事件监听与执行器
#[derive(Parser, Debug)]
#[command(name = "agent-loop", version, about)]
struct Args {
    /// Smee 频道 URL（快速模式，不读取配置文件）
    #[arg(short = 'u', long)]
    url: Option<String>,

    /// 基础提示词（快速模式）
    #[arg(short = 'p', long)]
    prompt: Option<String>,

    /// 汇报器名称
    #[arg(short = 'r', long, default_value = "console")]
    reporter: String,

    /// 工作区目录（快速模式，默认当前目录）
    #[arg(short = 'w', long)]
    workspace: Option<String>,

    /// 配置文件路径（覆盖默认 ~/.agent-loop/config.json）
    #[arg(short = 'c', long)]
    config: Option<String>,

    /// 启用并发执行（默认串行）
    #[arg(long)]
    concurrent: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // 加载 .env 文件（如果存在）
    let _ = dotenvy::dotenv();

    let args = Args::parse();

    // 同步 CLI flags 到环境变量
    if args.concurrent {
        std::env::set_var("LOOP_CONCURRENT", "true");
    }
    if let Some(ref config_path) = args.config {
        std::env::set_var("LOOP_CONFIG", config_path);
    }

    let concurrent = std::env::var("LOOP_CONCURRENT")
        .map(|v| v.to_lowercase() == "true")
        .unwrap_or(false);

    println!("🔁 Agent Loop 启动中...\n");

    let subscriptions: Vec<SubscriptionConfig>;
    let default_workspace: String;

    if let Some(url) = args.url {
        // 快速模式
        let Some(prompt) = args.prompt else {
            eprintln!("❌ 快速模式下必须提供 -p/--prompt 参数");
            std::process::exit(1);
        };
        let ws = args
            .workspace
            .unwrap_or_else(|| std::env::current_dir().unwrap().to_string_lossy().into_owned());
        println!("⚡ 快速模式 | URL: {url}");
        default_workspace = ws.clone();
        subscriptions = vec![SubscriptionConfig {
            name: "quick".to_string(),
            smee_url: url,
            enabled: true,
            base_prompt: prompt,
            workspace: Some(ws),
            reporter: Some(args.reporter.clone()),
        }];
    } else {
        // 配置文件模式
        let config = config::load_config()?;
        let dir = config::agent_loop_dir();
        println!("📁 配置目录: {}", dir.display());
        println!("📁 默认工作区: {}", config.default_workspace);

        subscriptions = config.subscriptions.into_iter().filter(|s| s.enabled).collect();
        default_workspace = config.default_workspace;

        if subscriptions.is_empty() {
            eprintln!(
                "⚠️  没有已启用的订阅。\n   请编辑 {}，添加订阅并将 enabled 设为 true。",
                dir.join("config.json").display()
            );
            return Ok(());
        }
    }

    println!(
        "⚙️  执行模式: {}",
        if concurrent { "并发" } else { "串行（默认）" }
    );
    println!("📋 活跃订阅数: {}\n", subscriptions.len());

    // 初始化汇报器并启动所有订阅
    let mut handles = Vec::new();
    for sub in subscriptions {
        let reporter_name = sub.reporter.clone();
        let reporter = get_reporter(reporter_name.as_deref());
        reporter.initialize().await?;

        let ws = default_workspace.clone();
        let handle = tokio::spawn(async move {
            subscriber::start_subscriber(sub, ws, reporter, concurrent).await;
        });
        handles.push(handle);
    }

    println!("✅ 所有订阅已启动，等待事件中...");
    println!("按 Ctrl+C 退出\n");

    // 等待 Ctrl+C 或 SIGTERM
    tokio::select! {
        _ = signal::ctrl_c() => {
            println!("\n\n🛑 收到 SIGINT 信号，正在关闭...");
        }
        _ = async {
            #[cfg(unix)]
            {
                let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("无法注册 SIGTERM 处理器");
                sigterm.recv().await;
            }
            #[cfg(not(unix))]
            {
                // Windows 上只等 ctrl_c，此分支永不触发
                std::future::pending::<()>().await;
            }
        } => {
            println!("\n\n🛑 收到 SIGTERM 信号，正在关闭...");
        }
    }

    for handle in handles {
        handle.abort();
    }
    println!("👋 Agent Loop 已退出");
    Ok(())
}
