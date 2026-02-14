pub mod anthropic;
pub mod openai;

use async_trait::async_trait;

/// Trait for LLM providers
#[async_trait]
#[allow(dead_code)]
pub trait LlmProvider: Send + Sync {
    async fn generate(&self, request: GenerationRequest) -> anyhow::Result<GenerationResponse>;
    fn name(&self) -> &str;
}

/// Request to an LLM provider
#[derive(Debug)]
#[allow(dead_code)]
pub struct GenerationRequest {
    pub system_prompt: String,
    pub context: String,
    pub user_prompt: String,
    pub model: String,
    pub temperature: f64,
    pub seed: Option<u64>,
}

/// Response from an LLM provider
#[derive(Debug)]
pub struct GenerationResponse {
    pub content: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub model: String,
}
