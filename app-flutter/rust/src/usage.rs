//! 用量日志 + 定价表（v1 `storage.rs` 的 A12 部分，2026-11-14 移植）。
//!
//! 用量日志：`<exe 根>\ModuleData\usage\usage-YYYY-MM-DD.jsonl`，**一行 = 一次提问**
//! （含提问内所有工具往返）。只追加不重写；文件名用本地日期（由前端传入，Rust 没有时区库）。
//!
//! 定价表：`<exe 根>\config\pricing.json`，单位**元 / 百万 token**，四类分别计价
//! （`input` / `cache_read` / `cache_write` / `output`）。价格不写进代码 —— 各家单价差十倍、
//! 官方还会调价，写死一个数字等于把错误金额当事实展示。缺文件时落一份 v1 预置骨架
//! （`DEFAULT_PRICING_JSON` 与 v1 `storage.rs` 逐字一致）。
//!
//! 候选价格：`<工作目录>/lunac-pricing.pending.json`（agent 只能写这里，用户在面板上
//! 预览「旧值 → 新值」并确认后才覆盖正式文件）。确认 = **先校验、后覆盖**，不合法一个字
//! 都不写；成功后删掉候选文件。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── 用量日志 ──────────────────────────────────────────────────────

/// 单次 API 请求的用量明细（`requests` 数组的一项）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UsageRequest {
    /// 该次请求未命中缓存的输入 token
    #[serde(rename = "in", default)]
    pub input: u64,
    /// 命中缓存的输入 token
    #[serde(default)]
    pub read: u64,
    /// 缓存写入 token
    #[serde(default)]
    pub create: u64,
    /// 该次请求的输出 token
    #[serde(default)]
    pub out: u64,
}

/// 一次提问的用量记录（与 v1 `storage::UsageRecord` 字段名逐字一致）。
///
/// `subagent` / `review` 只作归因（**是顶层四类的一部分，不得再加一次**），
/// 本模块原样透传，不参与汇总。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    /// 本地时钟的 epoch 毫秒
    pub ts: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    /// `"pet"` = 桌宠发起的那一轮；空串 = 普通聊天
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    #[serde(default)]
    pub elided: u64,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<UsageRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

/// 某个模型在一天里**某一个本地小时**的合计（分时价用）。
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageHourTotals {
    /// 本地小时 0–23
    pub hour: u32,
    pub turns: u64,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
}

/// 某个模型在一天里的合计（决定用哪一档单价）。
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelTotals {
    pub model: String,
    pub source: String,
    pub turns: u64,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    /// 按本地小时拆开的同一批量（与上面四个字段是同一批 token）
    pub hours: Vec<UsageHourTotals>,
}

/// 一天的合计（面板「按天表格」的一行）。
#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    pub date: String,
    pub turns: u64,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
    pub models: Vec<UsageModelTotals>,
}

fn usage_dir(root: &Path) -> PathBuf {
    root.join("ModuleData").join("usage")
}

fn usage_log_path(root: &Path, date: &str) -> Result<PathBuf, String> {
    validate_iso_date(date)?;
    Ok(usage_dir(root).join(format!("usage-{date}.jsonl")))
}

/// 严格 `YYYY-MM-DD`（日期一律来自前端，必须挡住路径拼串）。
fn validate_iso_date(date: &str) -> Result<(), String> {
    let b = date.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !ok {
        return Err(format!("日期不合法：{date}（要 YYYY-MM-DD）"));
    }
    Ok(())
}

/// 追加一条用量记录（只追加，失败由调用方忽略 —— 记账不该影响对话）。
pub fn append(root: &Path, date: &str, record: &serde_json::Value) -> Result<(), String> {
    let path = usage_log_path(root, date)?;
    let parsed: UsageRecord =
        serde_json::from_value(record.clone()).map_err(|e| format!("用量记录不合法：{e}"))?;
    std::fs::create_dir_all(usage_dir(root)).map_err(|e| e.to_string())?;
    let line = serde_json::to_string(&parsed).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())
}

/// 读某天的全部记录；文件不存在返回空表。单行损坏只跳过该行。
pub fn read_log(root: &Path, date: &str) -> Vec<UsageRecord> {
    let Ok(path) = usage_log_path(root, date) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<UsageRecord>(line).ok())
        .collect()
}

/// 读多天并汇总成「按天 + 按模型（+ 按本地小时）」的形状（v1 `read_usage_range` 同口径）。
///
/// `utc_offset_minutes` = 本地时区相对 UTC 的偏移（东八区传 480）。
pub fn read_range(root: &Path, dates: &[String], utc_offset_minutes: i32) -> Vec<UsageDay> {
    let offset_ms = (utc_offset_minutes as i64) * 60_000;
    let mut days = Vec::new();
    for date in dates {
        let records = read_log(root, date);
        if records.is_empty() {
            continue;
        }
        let mut day = UsageDay {
            date: date.clone(),
            ..Default::default()
        };
        // 键是 (来源, 模型)：同一模型名下桌宠 / 普通聊天各自独立成桶。
        let mut by_model: BTreeMap<(String, String), UsageModelTotals> = BTreeMap::new();
        let mut hours_by_model: BTreeMap<(String, String), BTreeMap<u32, UsageHourTotals>> =
            BTreeMap::new();
        for record in records {
            // 旧记录没有 `requests` 数组 ⇒ 按 1 计（一条记录至少对应一次 API 请求）。
            let requests = record.requests.len().max(1) as u64;
            day.turns += 1;
            day.requests += requests;
            day.input += record.input;
            day.output += record.output;
            day.cache_read += record.cache_read;
            day.cache_create += record.cache_create;
            let key = (record.source.clone(), record.model.clone());
            let model = by_model
                .entry(key.clone())
                .or_insert_with(|| UsageModelTotals {
                    model: record.model.clone(),
                    source: record.source.clone(),
                    ..Default::default()
                });
            model.turns += 1;
            model.requests += requests;
            model.input += record.input;
            model.output += record.output;
            model.cache_read += record.cache_read;
            model.cache_create += record.cache_create;

            let hour = ((record.ts as i64 + offset_ms).div_euclid(3_600_000)).rem_euclid(24) as u32;
            let bucket = hours_by_model
                .entry(key)
                .or_default()
                .entry(hour)
                .or_insert_with(|| UsageHourTotals {
                    hour,
                    ..Default::default()
                });
            bucket.turns += 1;
            bucket.requests += requests;
            bucket.input += record.input;
            bucket.output += record.output;
            bucket.cache_read += record.cache_read;
            bucket.cache_create += record.cache_create;
        }
        for model in by_model.values_mut() {
            if let Some(buckets) = hours_by_model.remove(&(model.source.clone(), model.model.clone()))
            {
                model.hours = buckets.into_values().collect();
            }
        }
        day.models = by_model.into_values().collect();
        days.push(day);
    }
    days
}

// ── 定价表 ────────────────────────────────────────────────────────

/// v1 `storage.rs` 的预置定价表（2026-09-29 用户批准预置；两处独立证据逐项对上）。
/// **与 v1 逐字一致**，只在文件不存在时落一次。
const DEFAULT_PRICING_JSON: &str = r#"{
  "updated_at": "2026-09-29",
  "holidays": [
    "2026-01-01", "2026-01-02", "2026-01-03",
    "2026-02-15", "2026-02-16", "2026-02-17", "2026-02-18", "2026-02-19",
    "2026-02-20", "2026-02-21", "2026-02-22", "2026-02-23",
    "2026-04-04", "2026-04-05", "2026-04-06",
    "2026-05-01", "2026-05-02", "2026-05-03", "2026-05-04", "2026-05-05",
    "2026-06-19", "2026-06-20", "2026-06-21",
    "2026-09-25", "2026-09-26", "2026-09-27",
    "2026-10-01", "2026-10-02", "2026-10-03", "2026-10-04",
    "2026-10-05", "2026-10-06", "2026-10-07"
  ],
  "models": {
    "deepseek-v4-flash": {
      "input": 1,
      "cache_read": 0.02,
      "cache_write": 0,
      "output": 4,
      "source_url": "https://api-docs.deepseek.com/quick_start/pricing",
      "updated_at": "2026-09-29",
      "time_windows": [
        {
          "days": [1, 2, 3, 4, 5],
          "from": "09:00",
          "to": "12:00",
          "input": 2,
          "cache_read": 0.04,
          "cache_write": 0,
          "output": 8
        },
        {
          "days": [1, 2, 3, 4, 5],
          "from": "14:00",
          "to": "18:00",
          "input": 2,
          "cache_read": 0.04,
          "cache_write": 0,
          "output": 8
        }
      ]
    }
  }
}
"#;

pub fn pricing_path(root: &Path) -> PathBuf {
    root.join("config").join("pricing.json")
}

fn pending_path(root: &Path) -> PathBuf {
    crate::sandbox::workdir(root).join("lunac-pricing.pending.json")
}

/// 面板要的一份状态：正式表 / 候选表的路径与原文（读不出就是空串，不报错）。
pub fn pricing_state_json(root: &Path) -> String {
    let path = pricing_path(root);
    let pending = pending_path(root);
    serde_json::json!({
        "path": path.display().to_string(),
        "text": std::fs::read_to_string(&path).unwrap_or_default(),
        "pendingPath": pending.display().to_string(),
        "pendingText": std::fs::read_to_string(&pending).unwrap_or_default(),
    })
    .to_string()
}

/// 缺文件时落预置骨架；老文件缺顶层 `holidays` 时补一次（其余字段一个都不动）。
pub fn ensure_pricing_file(root: &Path) -> Result<PathBuf, String> {
    let path = pricing_path(root);
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, DEFAULT_PRICING_JSON).map_err(|e| e.to_string())?;
    } else {
        backfill_pricing_holidays(&path);
    }
    Ok(path)
}

fn backfill_pricing_holidays(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
    else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if object.contains_key("holidays") {
        return;
    }
    let Some(holidays) = serde_json::from_str::<serde_json::Value>(DEFAULT_PRICING_JSON)
        .ok()
        .and_then(|default| default.get("holidays").cloned())
    else {
        return;
    };
    object.insert("holidays".into(), holidays);
    let Ok(out) = serde_json::to_string_pretty(&value) else {
        return;
    };
    if validate_pricing_text(&out).is_ok() {
        let _ = std::fs::write(path, format!("{out}\n"));
    }
}

/// 校验一份定价表文本（用户手写与 agent 候选走同一处）。
pub fn validate_pricing_text(text: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("语法错误：{e}"))?;
    let object = value.as_object().ok_or("顶层必须是一个对象")?;
    if let Some(holidays) = object.get("holidays") {
        validate_holidays_field("顶层", holidays)?;
    }
    let models = match object.get("models") {
        None => return Ok(()),
        Some(models) => models.as_object().ok_or("`models` 必须是一个对象")?,
    };
    for (name, entry) in models {
        let entry = entry
            .as_object()
            .ok_or_else(|| format!("模型 `{name}` 的值必须是一个对象"))?;
        validate_price_fields(&format!("模型 `{name}`"), entry)?;
        if let Some(holidays) = entry.get("holidays") {
            validate_holidays_field(&format!("模型 `{name}`"), holidays)?;
        }
        let Some(windows) = entry.get("time_windows") else {
            continue;
        };
        let array = windows
            .as_array()
            .ok_or_else(|| format!("模型 `{name}` 的 `time_windows` 必须是一个数组"))?;
        for (i, window) in array.iter().enumerate() {
            let what = format!("模型 `{name}` 的 `time_windows[{i}]`");
            let window = window
                .as_object()
                .ok_or_else(|| format!("{what} 必须是一个对象"))?;
            validate_price_fields(&what, window)?;
            let from = window
                .get("from")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_hhmm)
                .ok_or_else(|| format!("{what} 的 `from` 必须是 `HH:MM`（如 09:00）"))?;
            let to = window
                .get("to")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_hhmm)
                .ok_or_else(|| format!("{what} 的 `to` 必须是 `HH:MM`（如 12:00）"))?;
            if from >= to {
                return Err(format!(
                    "{what} 必须满足 `from` 早于 `to`（跨午夜请拆成两条，如 22:00-24:00 与 00:00-06:00）"
                ));
            }
            if let Some(days) = window.get("days") {
                let days = days
                    .as_array()
                    .ok_or_else(|| format!("{what} 的 `days` 必须是一个数组"))?;
                if days.is_empty() {
                    return Err(format!("{what} 的 `days` 不能是空数组（省略它表示每天）"));
                }
                let mut seen = [false; 8];
                for day in days {
                    let n = day
                        .as_u64()
                        .ok_or_else(|| format!("{what} 的 `days` 只能是 1–7 的整数"))?;
                    if !(1..=7).contains(&n) {
                        return Err(format!("{what} 的 `days` 只能是 1–7（1=周一 … 7=周日）"));
                    }
                    if seen[n as usize] {
                        return Err(format!("{what} 的 `days` 里 `{n}` 重复了"));
                    }
                    seen[n as usize] = true;
                }
            }
        }
    }
    Ok(())
}

fn validate_price_fields(
    what: &str,
    entry: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for field in ["input", "cache_read", "cache_write", "output"] {
        let number = entry
            .get(field)
            .ok_or_else(|| format!("{what} 缺少 `{field}`"))?
            .as_f64()
            .ok_or_else(|| format!("{what} 的 `{field}` 必须是数字"))?;
        if !number.is_finite() || number < 0.0 {
            return Err(format!("{what} 的 `{field}` 必须是非负数字"));
        }
    }
    Ok(())
}

fn validate_holidays_field(what: &str, value: &serde_json::Value) -> Result<(), String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{what} 的 `holidays` 必须是一个数组"))?;
    for (i, date) in array.iter().enumerate() {
        let text = date
            .as_str()
            .ok_or_else(|| format!("{what} 的 `holidays[{i}]` 必须是 `YYYY-MM-DD` 字符串"))?;
        validate_iso_date(text)
            .map_err(|_| format!("{what} 的 `holidays[{i}]` 不是合法的 `YYYY-MM-DD`：{text}"))?;
    }
    Ok(())
}

fn parse_hhmm(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    let digit = |i: usize| -> Option<u32> {
        bytes[i].is_ascii_digit().then(|| (bytes[i] - b'0') as u32)
    };
    let hour = digit(0)? * 10 + digit(1)?;
    let minute = digit(3)? * 10 + digit(4)?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(hour * 60 + minute)
}

/// 把候选价格落成正式表：**先校验、后覆盖**，成功后删掉候选文件。
pub fn commit_pricing_pending(root: &Path, today: &str) -> Result<PathBuf, String> {
    let pending = pending_path(root);
    let text = std::fs::read_to_string(&pending)
        .map_err(|e| format!("读不出待确认价格 {}：{e}", pending.display()))?;
    validate_pricing_text(&text).map_err(|e| format!("待确认价格不合法（原文件未改动）：{e}"))?;
    validate_iso_date(today)?;
    let mut value: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    value["updated_at"] = serde_json::json!(today);
    let out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    let path = pricing_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, format!("{out}\n")).map_err(|e| e.to_string())?;
    clear_pricing_pending(root)?;
    Ok(path)
}

/// 删除候选价格文件（不存在视为已删除）。
pub fn clear_pricing_pending(root: &Path) -> Result<(), String> {
    let pending = pending_path(root);
    match std::fs::remove_file(&pending) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删不掉 {}：{e}", pending.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunac-usage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn append_and_read_range_group_by_model_source_and_hour() {
        let root = temp_root("range");
        // 两个固定时间戳：相隔 2 小时，保证落在两个不同的本地小时桶。
        let base = 1_763_046_600_000u64;
        let second = base + 120 * 60_000;
        let record = |ts: u64, source: &str, requests: u64| {
            serde_json::json!({
                "ts": ts,
                "model": "deepseek-v4-flash",
                "sessionId": "s1",
                "source": source,
                "input": 10,
                "output": 20,
                "cacheRead": 30,
                "cacheCreate": 0,
                "requests": (0..requests).map(|_| serde_json::json!({"in": 5, "read": 15, "create": 0, "out": 10})).collect::<Vec<_>>(),
            })
        };
        append(&root, "2026-11-14", &record(base, "", 2)).expect("append");
        append(&root, "2026-11-14", &record(second, "", 0)).expect("append");
        append(&root, "2026-11-14", &record(second, "pet", 0)).expect("append");
        // 坏行不影响其它行
        std::fs::OpenOptions::new()
            .append(true)
            .open(usage_log_path(&root, "2026-11-14").unwrap())
            .unwrap()
            .write_all(b"{broken}\n")
            .unwrap();

        let days = read_range(&root, &["2026-11-14".to_string()], 480);
        assert_eq!(days.len(), 1);
        let day = &days[0];
        assert_eq!(day.turns, 3);
        assert_eq!(day.requests, 4, "旧记录 requests 缺失时按 1 计");
        assert_eq!(day.input, 30);
        // 空来源排在前面（BTreeMap 按 (source, model) 升序）
        assert_eq!(day.models.len(), 2);
        assert_eq!(day.models[0].source, "");
        assert_eq!(day.models[0].turns, 2);
        assert_eq!(day.models[1].source, "pet");
        assert_eq!(day.models[0].hours.len(), 2, "两条记录落在两个本地小时");
        let local_hour = |ts: u64| {
            ((ts as i64 + 480 * 60_000).div_euclid(3_600_000)).rem_euclid(24) as u32
        };
        let mut hours: Vec<u32> = day.models[0].hours.iter().map(|h| h.hour).collect();
        hours.sort_unstable();
        let mut expected = vec![local_hour(base), local_hour(second)];
        expected.sort_unstable();
        assert_eq!(hours, expected);

        let days = read_range(&root, &["2026-13-01".to_string()], 480);
        assert!(days.is_empty(), "非法日期读成空表，不 panic");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn append_rejects_bad_dates_and_bad_records() {
        let root = temp_root("reject");
        let record = serde_json::json!({"ts": 1, "model": "m", "input": 0, "output": 0, "cacheRead": 0, "cacheCreate": 0});
        assert!(append(&root, "../../evil", &record).is_err());
        assert!(append(&root, "2026-11-14", &serde_json::json!({"ts": 1})).is_err());
        assert!(append(&root, "2026-11-14", &record).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_pricing_json_is_valid_and_has_the_v1_model() {
        validate_pricing_text(DEFAULT_PRICING_JSON).expect("预置价表必须合法");
        let value: serde_json::Value = serde_json::from_str(DEFAULT_PRICING_JSON).unwrap();
        assert!(value["models"]["deepseek-v4-flash"]["time_windows"]
            .as_array()
            .is_some_and(|windows| windows.len() == 2));
        assert!(value["holidays"].as_array().is_some_and(|h| !h.is_empty()));
    }

    #[test]
    fn pricing_round_trip_and_commit_requires_valid_pending() {
        let root = temp_root("pricing");
        let path = ensure_pricing_file(&root).expect("ensure");
        assert!(path.exists(), "缺文件时落预置骨架");

        let pending = pending_path(&root);
        std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
        std::fs::write(&pending, DEFAULT_PRICING_JSON).unwrap();
        let state = pricing_state_json(&root);
        assert!(state.contains("pendingText"), "{state}");

        // 不合法的候选：一个字都不写，候选文件保留
        std::fs::write(&pending, "{\"models\":{\"x\":{\"input\":1}}}").unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(commit_pricing_pending(&root, "2026-11-14").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(pending.exists());

        std::fs::write(&pending, DEFAULT_PRICING_JSON).unwrap();
        commit_pricing_pending(&root, "2026-11-14").expect("commit");
        assert!(!pending.exists(), "确认后删掉候选文件");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"updated_at\": \"2026-11-14\""), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pending_path_lives_in_the_sandbox_workdir() {
        let root = Path::new("D:\\Lunac");
        let pending = pending_path(root);
        assert!(pending.ends_with("lunac-pricing.pending.json"));
        assert!(pending.display().to_string().contains("transStorage"));
    }
}
