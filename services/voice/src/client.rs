//! HTTP client for the local Rust voice sidecar.
use anyhow::{Context, Result};
use reqwest::Client;
use std::path::Path;

use crate::types::{
    HealthResponse, ModelsResponse, TTSRequest, TTSResponse, VCRequest, VCResponse, UploadResponse,
};

#[derive(Debug)]
pub struct VoiceClient {
    base_url: String,
    client: Client,
}

impl Default for VoiceClient {
    fn default() -> Self {
        Self::new("http://localhost:8001".to_string())
    }
}

impl VoiceClient {
    pub fn new(base_url: String) -> Self {
        let client = Client::builder()
            .no_proxy()
            .build()
            .expect("Failed to build reqwest client");
        Self {
            base_url,
            client,
        }
    }


    pub async fn health_check(&self) -> Result<HealthResponse> {
        let url = format!("{}/health", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send health check request")?;

        if !response.status().is_success() {
            anyhow::bail!("Health check failed: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse health check response")
    }

    pub async fn list_models(&self) -> Result<ModelsResponse> {
        let url = format!("{}/v1/voice/models", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send list models request")?;

        if !response.status().is_success() {
            anyhow::bail!("List models failed: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse list models response")
    }

    pub async fn text_to_speech(&self, request: TTSRequest) -> Result<TTSResponse> {
        let url = format!("{}/v1/voice/tts", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send TTS request")?;

        if !response.status().is_success() {
            anyhow::bail!("TTS failed: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse TTS response")
    }

    pub async fn voice_clone(&self, request: VCRequest) -> Result<VCResponse> {
        let url = format!("{}/v1/voice/clone", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send voice clone request")?;

        if !response.status().is_success() {
            anyhow::bail!("Voice clone failed: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse voice clone response")
    }

    pub async fn upload_reference_audio<P: AsRef<Path>>(
        &self,
        audio_path: P,
    ) -> Result<UploadResponse> {
        let url = format!("{}/v1/voice/upload", self.base_url);

        let audio_path_ref = audio_path.as_ref();

        let audio_bytes = tokio::fs::read(audio_path_ref)
            .await
            .context("Failed to read audio file")?;

        let file_part = reqwest::multipart::Part::bytes(audio_bytes)
            .file_name(
                audio_path_ref
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("audio.wav")
                    .to_string()
            )
            .mime_str("audio/wav")
            .context("Failed to set MIME type")?;

        let form = reqwest::multipart::Form::new().part("file", file_part);

        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("Failed to send upload request")?;

        if !response.status().is_success() {
            anyhow::bail!("Upload failed: {}", response.status());
        }

        response
            .json()
            .await
            .context("Failed to parse upload response")
    }

    pub fn get_audio_url(&self, filename: &str) -> String {
        format!("{}/v1/voice/audio/{}", self.base_url, filename)
    }
}
