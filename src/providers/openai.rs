use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{GenerationRequest, GenerationResponse, LlmProvider};

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";
const MAX_TOKENS: u64 = 16384;

/// OpenAI API provider (GPT-4o, GPT-4, etc.)
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }
}

// ---------- API request/response types ----------

#[derive(Debug, Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u64,
    messages: Vec<ApiMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    choices: Vec<Choice>,
    model: String,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[serde(rename = "type")]
    error_type: Option<String>,
}

// ---------- LlmProvider implementation ----------

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn generate(&self, request: GenerationRequest) -> Result<GenerationResponse> {
        let mut messages = Vec::new();

        // System message
        messages.push(ApiMessage {
            role: "system".to_string(),
            content: request.system_prompt.clone(),
        });

        // User message: prompt body + context
        let user_content = if request.context.is_empty() {
            request.user_prompt.clone()
        } else {
            format!(
                "{}\n\n---\n\n## Context (generated code from imported prompts)\n\n{}\n",
                request.user_prompt, request.context
            )
        };

        messages.push(ApiMessage {
            role: "user".to_string(),
            content: user_content,
        });

        let api_request = ApiRequest {
            model: request.model.clone(),
            max_tokens: MAX_TOKENS,
            messages,
            temperature: request.temperature,
            seed: request.seed,
        };

        let response = self
            .client
            .post(OPENAI_API_URL)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await
            .context("Failed to send request to OpenAI API")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read OpenAI API response body")?;

        if !status.is_success() {
            if let Ok(api_error) = serde_json::from_str::<ApiError>(&body) {
                let error_type = api_error
                    .error
                    .error_type
                    .as_deref()
                    .unwrap_or("unknown");

                match error_type {
                    "authentication_error" | "invalid_api_key" => {
                        bail!(
                            "OpenAI API authentication failed. Check your API key.\n  {}",
                            api_error.error.message
                        );
                    }
                    "rate_limit_error" | "rate_limit_exceeded" => {
                        bail!(
                            "OpenAI API rate limit hit. Try again in a moment.\n  {}",
                            api_error.error.message
                        );
                    }
                    "server_error" => {
                        bail!(
                            "OpenAI API server error. Try again shortly.\n  {}",
                            api_error.error.message
                        );
                    }
                    _ => {
                        bail!(
                            "OpenAI API error ({}): {}",
                            error_type,
                            api_error.error.message
                        );
                    }
                }
            }

            bail!(
                "OpenAI API returned HTTP {}: {}",
                status,
                &body[..body.len().min(500)]
            );
        }

        let api_response: ApiResponse = serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse OpenAI API response: {}",
                &body[..body.len().min(200)]
            )
        })?;

        let content = api_response
            .choices
            .first()
            .and_then(|c| c.message.content.as_deref())
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            bail!(
                "OpenAI API returned empty response (choices: {})",
                api_response.choices.len()
            );
        }

        Ok(GenerationResponse {
            content,
            tokens_in: api_response.usage.prompt_tokens,
            tokens_out: api_response.usage.completion_tokens,
            model: api_response.model,
        })
    }

    fn name(&self) -> &str {
        "openai"
    }
}
