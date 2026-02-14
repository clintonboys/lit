use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{GenerationRequest, GenerationResponse, LlmProvider};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u64 = 16384;

/// Anthropic API provider (Claude)
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
}

impl AnthropicProvider {
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
    system: String,
    messages: Vec<ApiMessage>,
    temperature: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<ApiMetadata>,
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ApiMetadata {
    user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    model: String,
    usage: Usage,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ---------- LlmProvider implementation ----------

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn generate(&self, request: GenerationRequest) -> Result<GenerationResponse> {
        let user_content = if request.context.is_empty() {
            request.user_prompt.clone()
        } else {
            format!(
                "{}\n\n---\n\n## Context (generated code from imported prompts)\n\n{}\n",
                request.user_prompt, request.context
            )
        };

        let api_request = ApiRequest {
            model: request.model.clone(),
            max_tokens: MAX_TOKENS,
            system: request.system_prompt.clone(),
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: user_content,
            }],
            temperature: request.temperature,
            metadata: None,
        };

        let response = self
            .client
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&api_request)
            .send()
            .await
            .context("Failed to send request to Anthropic API")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read Anthropic API response body")?;

        if !status.is_success() {
            // Try to parse error details
            if let Ok(api_error) = serde_json::from_str::<ApiError>(&body) {
                match api_error.error.error_type.as_str() {
                    "authentication_error" => {
                        bail!(
                            "Anthropic API authentication failed. Check your API key.\n  {}",
                            api_error.error.message
                        );
                    }
                    "rate_limit_error" => {
                        bail!(
                            "Anthropic API rate limit hit. Try again in a moment.\n  {}",
                            api_error.error.message
                        );
                    }
                    "overloaded_error" => {
                        bail!(
                            "Anthropic API is overloaded. Try again shortly.\n  {}",
                            api_error.error.message
                        );
                    }
                    _ => {
                        bail!(
                            "Anthropic API error ({}): {}",
                            api_error.error.error_type,
                            api_error.error.message
                        );
                    }
                }
            }

            bail!(
                "Anthropic API returned HTTP {}: {}",
                status,
                &body[..body.len().min(500)]
            );
        }

        let api_response: ApiResponse = serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse Anthropic API response: {}",
                &body[..body.len().min(200)]
            )
        })?;

        // Extract text from content blocks
        let content = api_response
            .content
            .iter()
            .filter(|block| block.block_type == "text")
            .filter_map(|block| block.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        if content.is_empty() {
            bail!(
                "Anthropic API returned empty response (stop_reason: {:?})",
                api_response.stop_reason
            );
        }

        Ok(GenerationResponse {
            content,
            tokens_in: api_response.usage.input_tokens,
            tokens_out: api_response.usage.output_tokens,
            model: api_response.model,
        })
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}
