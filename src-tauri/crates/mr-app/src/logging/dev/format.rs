use log::Record;

use super::filter::DevConfig;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Text,
    Jsonl,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PathMode {
    Redacted,
    Full,
}

pub(super) fn escape_value(value: &str) -> String {
    if value
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        )
    } else {
        value.to_string()
    }
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '\t'], " ")
}

fn timestamp() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

struct KvCollector {
    fields: Vec<(String, serde_json::Value)>,
}

impl<'kvs> log::kv::VisitSource<'kvs> for KvCollector {
    fn visit_pair(
        &mut self,
        key: log::kv::Key<'kvs>,
        value: log::kv::Value<'kvs>,
    ) -> Result<(), log::kv::Error> {
        self.fields.push((key.to_string(), json_value(value)));
        Ok(())
    }
}

fn display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn split_event(args: &str) -> (Option<&str>, &str) {
    if let Some(rest) = args.strip_prefix("ev=") {
        match rest.split_once(char::is_whitespace) {
            Some((event, message)) => (Some(event), message.trim_start()),
            None => (Some(rest), ""),
        }
    } else {
        (None, args)
    }
}

fn json_value(value: log::kv::Value<'_>) -> serde_json::Value {
    if let Some(v) = value.to_bool() {
        return v.into();
    }
    if let Some(v) = value.to_i64() {
        return v.into();
    }
    if let Some(v) = value.to_u64() {
        return v.into();
    }
    if let Some(v) = value.to_f64() {
        return serde_json::Number::from_f64(v)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| value.to_string().into());
    }
    value.to_string().into()
}

pub(super) fn format_record(record: &Record<'_>, config: DevConfig) -> String {
    let args = one_line(&record.args().to_string());
    let (event, message) = split_event(&args);
    let mut collector = KvCollector { fields: Vec::new() };
    let _ = record.key_values().visit(&mut collector);
    let run = super::current_run_id().unwrap_or_default();
    let stage = super::current_stage();
    let level = record.level().to_string();
    match config.format {
        Format::Text => {
            let mut line = format!(
                "{} {:<5} {:<6} {}",
                timestamp(),
                level,
                record.target(),
                args
            );
            if !run.is_empty() {
                line.push_str(&format!(" run={}", escape_value(&run)));
            }
            if !stage.is_empty() {
                line.push_str(&format!(" stage={}", escape_value(&stage)));
            }
            for (key, value) in &collector.fields {
                line.push(' ');
                line.push_str(key);
                line.push('=');
                line.push_str(&escape_value(&display_value(value)));
            }
            line
        }
        Format::Jsonl => {
            let mut map = serde_json::Map::new();
            map.insert("ts".into(), timestamp().into());
            map.insert("level".into(), level.to_ascii_lowercase().into());
            map.insert("target".into(), record.target().into());
            if let Some(event) = event {
                map.insert("ev".into(), event.into());
            }
            if !message.is_empty() {
                map.insert("msg".into(), message.into());
            }
            if !run.is_empty() {
                map.insert("run".into(), run.into());
            }
            if !stage.is_empty() {
                map.insert("stage".into(), stage.into());
            }
            for (key, value) in collector.fields {
                map.insert(key, value);
            }
            serde_json::Value::Object(map).to_string()
        }
    }
}

pub(super) fn dropped_line(format: Format, count: u64) -> String {
    match format {
        Format::Text => format!(
            "{} {:<5} {:<6} ev=diag.dropped count={count}",
            timestamp(),
            "WARN",
            "diag"
        ),
        Format::Jsonl => serde_json::json!({
            "ts": timestamp(),
            "level": "warn",
            "target": "diag",
            "ev": "diag.dropped",
            "count": count,
        })
        .to_string(),
    }
}
