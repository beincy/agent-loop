pub mod console;

use anyhow::Result;
use async_trait::async_trait;

use crate::types::ExecutionResult;

#[async_trait]
pub trait Reporter: Send + Sync {
    fn name(&self) -> &str;
    async fn report(&self, result: &ExecutionResult) -> Result<()>;
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }
    async fn dispose(&self) -> Result<()> {
        Ok(())
    }
}

pub fn get_reporter(name: Option<&str>) -> Box<dyn Reporter> {
    match name.unwrap_or("console") {
        "console" => Box::new(console::ConsoleReporter::new()),
        other => {
            eprintln!("⚠️  未找到汇报器 \"{other}\"，回退到 \"console\" 汇报器");
            Box::new(console::ConsoleReporter::new())
        }
    }
}
