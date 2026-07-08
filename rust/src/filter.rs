#![allow(dead_code)]

/// 两阶段预处理：正则过滤 → WASM 策略插件
///
/// # WASM 策略插件规范
///
/// 文件位置：`~/.agent-loop/policy/<name>.wasm`
/// 导出函数：`process(ptr: i32, len: i32) -> i32`（及 alloc/memory）
///
/// 输入：完整事件 JSON（SmeeEventData）
/// 输出（输出指针处）：`{ "allow": true/false, "json"?: SmeeEventData }`
use anyhow::{Context, Result};
use regex::Regex;
use std::path::PathBuf;

use crate::types::SmeeEventData;
use crate::wasm_host::call_wasm;

#[derive(Debug)]
pub struct FilterResult {
    pub allow: bool,
    /// 可能被 WASM 插件修改过的事件
    pub event: SmeeEventData,
}

/// 依次执行正则过滤和 WASM 策略，任一拒绝则返回 allow=false
pub async fn apply_filters(
    event: SmeeEventData,
    filter_regex: Option<&str>,
    wasm_policy: Option<&str>,
) -> Result<FilterResult> {
    // ── 阶段 1：正则过滤（支持 `&&` 分隔的 AND 条件，最多 3 个）──
    if let Some(pattern) = filter_regex {
        let patterns: Vec<&str> = pattern
            .split("&&")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if patterns.len() > 3 {
            anyhow::bail!("正则过滤最多支持 3 个 AND 条件，当前 {} 个", patterns.len());
        }
        let event_json = serde_json::to_string(&event)?;
        for p in &patterns {
            let re = Regex::new(p).with_context(|| format!("正则表达式无效: {p}"))?;
            if !re.is_match(&event_json) {
                println!("   🚫 [正则过滤] 条件 `{p}` 未匹配，跳过此事件");
                return Ok(FilterResult { allow: false, event });
            }
        }
        if !patterns.is_empty() {
            println!("   ✅ [正则过滤] {} 个条件全部通过", patterns.len());
        }
    }

    // ── 阶段 2：WASM 策略插件 ──────────────────────────────────
    if let Some(plugin_name) = wasm_policy {
        let wasm_path = dirs::home_dir()
            .context("无法获取 Home 目录")?
            .join(".agent-loop")
            .join("policy")
            .join(format!("{plugin_name}.wasm"));

        if !wasm_path.exists() {
            return Err(anyhow::anyhow!(
                "WASM 策略插件不存在: {}",
                wasm_path.display()
            ));
        }

        let event_json = serde_json::to_string(&event)?;
        let plugin_name_owned = plugin_name.to_string();

        let output_json = tokio::task::spawn_blocking(move || {
            call_wasm(&wasm_path, "process", &event_json)
        })
        .await??;

        let output: serde_json::Value = serde_json::from_str(&output_json)
            .context("WASM 策略插件返回非合法 JSON")?;

        let allow = output
            .get("allow")
            .and_then(|v| v.as_bool())
            .context("WASM 输出必须包含布尔类型的 'allow' 字段")?;

        let icon = if allow { "✅" } else { "🚫" };
        let verdict = if allow { "允许" } else { "拒绝" };
        println!("   {icon} [WASM 策略 | {plugin_name_owned}] {verdict}");

        if !allow {
            return Ok(FilterResult { allow: false, event });
        }

        let final_event = match output.get("json").cloned() {
            Some(v) => serde_json::from_value(v).unwrap_or(event),
            None => event,
        };

        return Ok(FilterResult { allow: true, event: final_event });
    }

    Ok(FilterResult { allow: true, event })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_with_body(text: &str) -> SmeeEventData {
        SmeeEventData {
            headers: Default::default(),
            query: Default::default(),
            body: serde_json::json!({ "action": "opened", "text": text }),
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn and_conditions_all_must_match() {
        let event = event_with_body("hello @claudecode01 issue opened");
        // 两个条件都命中 → 放行
        let r = apply_filters(event.clone(), Some("@claudecode01 && opened"), None)
            .await
            .unwrap();
        assert!(r.allow);
        // 其中一个不命中 → 拒绝
        let r = apply_filters(event.clone(), Some("@claudecode01 && closed"), None)
            .await
            .unwrap();
        assert!(!r.allow);
        // 三个条件（上限）
        let r = apply_filters(event.clone(), Some("hello && opened && issue"), None)
            .await
            .unwrap();
        assert!(r.allow);
    }

    #[tokio::test]
    async fn more_than_three_conditions_is_error() {
        let event = event_with_body("x");
        let err = apply_filters(event, Some("a && b && c && d"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("最多支持 3 个"));
    }

    #[tokio::test]
    async fn single_pattern_still_works() {
        let event = event_with_body("only opened here");
        assert!(apply_filters(event.clone(), Some("opened"), None).await.unwrap().allow);
        assert!(!apply_filters(event, Some("closed"), None).await.unwrap().allow);
    }
}

#[allow(dead_code)]
pub fn policy_wasm_path(plugin_name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join(".agent-loop")
            .join("policy")
            .join(format!("{plugin_name}.wasm"))
    })
}
