use super::backend::{
    BackendError, BackendEvent, BackendInfo, CancelToken, GenerateRequest, Phase, SummaryBackend,
};

/// Детерминированный backend для контрактных тестов: без модели, runtime и сети.
pub struct FakeBackend {
    mode: &'static str,
}

impl FakeBackend {
    pub fn new(mode: &'static str) -> Self {
        Self { mode }
    }
}

impl SummaryBackend for FakeBackend {
    fn probe(&mut self) -> Result<BackendInfo, BackendError> {
        Ok(BackendInfo {
            runtime: "fake".to_string(),
            version: "fake".to_string(),
            commit: "fake".to_string(),
            model_id: super::config::MODEL_ID.to_string(),
            n_ctx: super::config::CONTEXT_TOKENS,
            threads: 2,
        })
    }

    fn generate(
        &mut self,
        req: &GenerateRequest,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(BackendEvent),
    ) -> Result<String, BackendError> {
        if self.mode == "model_load" {
            return Err(BackendError::Load("fake load failure".to_string()));
        }
        if self.mode == "invalid" {
            for pct in 10..=90 {
                cancel.check()?;
                on_event(BackendEvent {
                    phase: Phase::Generate,
                    tokens_done: pct,
                    tokens_total: 100,
                });
            }
            return Ok("no sections at all".to_string());
        }
        if self.mode == "thin" {
            return Ok("## Кратко\n".to_string());
        }
        let text = first_utterance_text(&req.prompt).unwrap_or_default();
        let titles: Vec<&str> = req
            .prompt
            .lines()
            .filter_map(|line| line.strip_prefix("## "))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let steps = if self.mode == "slow" { 100 } else { 10 };
        for step in 1..=steps {
            cancel.check()?;
            on_event(BackendEvent {
                phase: Phase::Generate,
                tokens_done: step,
                tokens_total: steps,
            });
            if self.mode == "slow" {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        if titles.is_empty() || text.is_empty() {
            return Ok("## Кратко\n- нет данных.\n".to_string());
        }
        let mut out = String::new();
        for title in titles {
            out.push_str("## ");
            out.push_str(title);
            out.push('\n');
            out.push_str("- ");
            out.push_str(&text);
            out.push('\n');
        }
        // Хвост любого вывода не должен содержать nonce (проверка §9).
        Ok(out)
    }

    fn shutdown(&mut self) {}
}

fn first_utterance_text(prompt: &str) -> Option<String> {
    let start = prompt.find("<<<DATA_")?;
    let marker_end = prompt[start..].find("_BEGIN>>>")? + start + "_BEGIN>>>".len();
    let rest = prompt.get(marker_end..)?;
    let end = rest.find("_END>>>")?;
    let body = rest.get(..end)?.trim();
    let json_end = body.rfind("<<<DATA_").unwrap_or(body.len());
    let json = body.get(..json_end)?.trim();
    let rows: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let text = rows
        .iter()
        .find_map(|row| row.get("text").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|t| !t.is_empty())?;
    Some(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_first_non_empty_utterance() {
        let prompt = "## Кратко\n<<<DATA_abc_BEGIN>>>\n[{\"i\":0,\"speaker\":\"S\",\"text\":\"Обсудили срок запуска.\"}]\n<<<DATA_abc_END>>>\n";
        assert_eq!(
            first_utterance_text(prompt).as_deref(),
            Some("Обсудили срок запуска.")
        );
        assert!(first_utterance_text("no data").is_none());
    }
}
