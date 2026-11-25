//! Language model adapter: turns the text read on a prescription into structured items.
//!
//! It speaks the Ollama chat API, which serves both a local Ollama (`http://localhost:11434`,
//! no key) and Ollama Cloud (`https://ollama.com`, with a key). The reply is constrained by a
//! JSON schema (`format`), the temperature is 0, and the system prompt lives in
//! `prompts/prescription-structure.md`. Prescription text is never logged.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

pub const SYSTEM_PROMPT: &str = include_str!("../prompts/prescription-structure.md");

#[derive(Deserialize, Serialize, ToSchema, Clone, Debug)]
pub struct Item {
    /// Passage exact de l’ordonnance, sans la posologie.
    pub source_text: String,
    /// Nom écrit, marque ou molécule.
    pub name: String,
    #[serde(default)]
    pub ingredients: Vec<String>,
    #[serde(default)]
    pub strengths: Vec<String>,
    pub form: Option<String>,
    pub boxes: Option<i32>,
    pub units: Option<i32>,
    /// Classe thérapeutique selon le modèle, en français.
    #[serde(default)]
    pub therapeutic_class: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Clone, Debug)]
pub struct Ignored {
    pub text: String,
    pub reason: String,
}

#[derive(Deserialize, Serialize, ToSchema, Clone, Debug)]
pub struct Structured {
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub ignored: Vec<Ignored>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Usage {
    pub model: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Durée côté modèle, en millisecondes.
    pub duration_ms: Option<u64>,
}

#[derive(Debug)]
pub enum LlmError {
    /// Down, rate limited, bad key, or timeout.
    Unavailable(String),
    /// Answered, but not with the expected JSON.
    InvalidAnswer(String),
}

pub struct Llm {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

/// The JSON schema the model must follow, mirrored from the prompt.
fn schema() -> Value {
    let nullable = |t: &str| json!({ "type": [t, "null"] });
    json!({
        "type": "object",
        "properties": {
            "items": { "type": "array", "items": {
                "type": "object",
                "properties": {
                    "source_text": { "type": "string" },
                    "name": { "type": "string" },
                    "ingredients": { "type": "array", "items": { "type": "string" } },
                    "strengths": { "type": "array", "items": { "type": "string" } },
                    "form": nullable("string"),
                    "boxes": nullable("integer"),
                    "units": nullable("integer"),
                    "therapeutic_class": nullable("string")
                },
                "required": ["source_text", "name", "ingredients", "strengths", "form", "boxes", "units", "therapeutic_class"]
            }},
            "ignored": { "type": "array", "items": {
                "type": "object",
                "properties": { "text": { "type": "string" }, "reason": { "type": "string" } },
                "required": ["text", "reason"]
            }}
        },
        "required": ["items", "ignored"]
    })
}

impl Llm {
    /// `LLM_MODEL` switches the adapter on. `LLM_BASE_URL` defaults to Ollama Cloud,
    /// `LLM_API_KEY` is required there and empty for a local Ollama.
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let model = var("LLM_MODEL")?;
        Some(Self::new(var("LLM_BASE_URL").unwrap_or_else(|| "https://ollama.com".into()), var("LLM_API_KEY"), model))
    }

    pub fn new(base_url: impl Into<String>, api_key: Option<String>, model: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("client HTTP");
        Self { http, base_url: base_url.into().trim_end_matches('/').to_owned(), api_key, model: model.into() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn structure(&self, text: &str) -> Result<(Structured, Usage), LlmError> {
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": schema(),
            "options": { "temperature": 0 },
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": format!("Texte de l'ordonnance :\n```\n{text}\n```") }
            ]
        });
        let mut attempt = 0;
        let res = loop {
            let mut req = self.http.post(format!("{}/api/chat", self.base_url)).json(&body);
            if let Some(key) = &self.api_key {
                req = req.bearer_auth(key);
            }
            match req.send().await {
                Ok(r) if r.status().is_success() => break r,
                Ok(r) => {
                    let status = r.status();
                    tracing::warn!(%status, attempt, "le modèle de langage a répondu en erreur");
                    if (status.as_u16() == 429 || status.is_server_error()) && attempt < 1 {
                        attempt += 1;
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                    return Err(LlmError::Unavailable(format!("{status}")));
                }
                Err(e) => return Err(LlmError::Unavailable(e.without_url().to_string())),
            }
        };
        let reply: ChatReply = res.json().await.map_err(|e| LlmError::InvalidAnswer(format!("réponse illisible ({e})")))?;
        let structured = parse_answer(&reply.message.content)?;
        let usage = Usage {
            model: reply.model.unwrap_or_else(|| self.model.clone()),
            prompt_tokens: reply.prompt_eval_count,
            completion_tokens: reply.eval_count,
            duration_ms: reply.total_duration.map(|ns| ns / 1_000_000),
        };
        Ok((structured, usage))
    }
}

/// Some models wrap the JSON in a code fence despite the instructions.
pub fn parse_answer(content: &str) -> Result<Structured, LlmError> {
    let trimmed = content.trim();
    let json = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(trimmed);
    serde_json::from_str(json).map_err(|e| LlmError::InvalidAnswer(format!("JSON invalide ({e})")))
}

#[derive(Deserialize)]
struct ChatReply {
    model: Option<String>,
    message: ChatMessage,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    total_duration: Option<u64>,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_in_a_code_fence_is_accepted() {
        let s = parse_answer("```json\n{\"items\":[],\"ignored\":[]}\n```").unwrap();
        assert!(s.items.is_empty());
        assert!(parse_answer("Voici la liste : Doliprane").is_err());
    }

    #[test]
    fn prompt_is_embedded() {
        assert!(SYSTEM_PROMPT.contains("N'invente rien"));
    }
}
