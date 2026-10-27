//! Native Log Service - OC-025
//!
//! Native Rust implementation of log aggregation and querying service.
//! Reads logs from `.logs/` directory and provides filtering, searching,
//! and streaming capabilities via HTTP REST API.

use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::broadcast;

/// Log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: String,  // INFO, WARN, ERROR, DEBUG
    pub source: String, // kernel, api, gateway, etc.
    pub message: String,
    pub metadata: Option<HashMap<String, String>>,
}

/// Log query request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogQuery {
    pub sources: Option<Vec<String>>,
    pub level: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub search: Option<String>,
    pub limit: Option<usize>,
}

/// Log operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogOperation {
    /// Query logs with filters
    QueryLogs { query: LogQuery },
    /// Tail logs (last N lines)
    TailLogs { sources: Vec<String>, lines: usize },
    /// Get available log sources
    GetSources,
    /// Stream logs (returns a receiver for SSE)
    StreamLogs {
        sources: Vec<String>,
        filter: Option<String>,
    },
}

/// Log service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogServiceConfig {
    pub logs_dir: PathBuf,
    pub max_entries: usize,
    pub enable_streaming: bool,
    pub stream_buffer_size: usize,
}

impl Default for LogServiceConfig {
    fn default() -> Self {
        Self {
            logs_dir: PathBuf::from("./.logs"),
            max_entries: 10000,
            enable_streaming: true,
            stream_buffer_size: 100,
        }
    }
}

/// Log service
pub struct LogService {
    config: LogServiceConfig,
    /// Broadcast channel for log streaming
    log_tx: broadcast::Sender<LogEntry>,
}

/// Log error types
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    #[error("Source not found: {0}")]
    SourceNotFound(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

impl From<std::io::Error> for LogError {
    fn from(error: std::io::Error) -> Self {
        LogError::IoError(error.to_string())
    }
}

impl From<serde_json::Error> for LogError {
    fn from(error: serde_json::Error) -> Self {
        LogError::SerializationError(error.to_string())
    }
}

impl LogService {
    /// Create a new log service with default configuration
    pub fn new() -> Self {
        let config = LogServiceConfig::default();
        let (log_tx, _) = broadcast::channel(config.stream_buffer_size);

        Self { config, log_tx }
    }

    /// Create a new log service with custom configuration
    pub fn with_config(config: LogServiceConfig) -> Self {
        let (log_tx, _) = broadcast::channel(config.stream_buffer_size);

        Self { config, log_tx }
    }

    /// Get the logs directory path
    pub fn logs_dir(&self) -> &PathBuf {
        &self.config.logs_dir
    }

    /// Get a subscriber for log streaming
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }

    /// Execute a log operation
    pub async fn execute(&self, operation: LogOperation) -> Result<serde_json::Value, LogError> {
        match operation {
            LogOperation::QueryLogs { query } => self.query_logs(query).await,
            LogOperation::TailLogs { sources, lines } => self.tail_logs(sources, lines).await,
            LogOperation::GetSources => self.get_sources().await,
            LogOperation::StreamLogs { sources, filter } => {
                // For streaming, we return the channel info
                // Actual streaming is handled by the HTTP layer
                self.get_stream_info(sources, filter).await
            }
        }
    }

    /// Query logs with filters
    async fn query_logs(&self, query: LogQuery) -> Result<serde_json::Value, LogError> {
        let mut entries = Vec::new();

        // Determine which sources to read
        let sources = if let Some(ref sources) = query.sources {
            sources.clone()
        } else {
            self.list_source_files().await?
        };

        for source in sources {
            let source_entries = self.read_log_file(&source).await?;
            entries.extend(source_entries);
        }

        // Apply filters
        let mut filtered: Vec<LogEntry> = entries
            .into_iter()
            .filter(|entry| {
                // Filter by level
                if let Some(ref level) = query.level {
                    if entry.level.to_lowercase() != level.to_lowercase() {
                        return false;
                    }
                }

                // Filter by time range
                if let Some(since) = query.since {
                    if entry.timestamp < since {
                        return false;
                    }
                }

                if let Some(until) = query.until {
                    if entry.timestamp > until {
                        return false;
                    }
                }

                // Filter by search term
                if let Some(ref search) = query.search {
                    let search_lower = search.to_lowercase();
                    if !entry.message.to_lowercase().contains(&search_lower)
                        && !entry.source.to_lowercase().contains(&search_lower)
                    {
                        return false;
                    }
                }

                true
            })
            .collect();

        // Sort by timestamp (newest first)
        filtered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Apply limit
        if let Some(limit) = query.limit {
            if filtered.len() > limit {
                filtered.truncate(limit);
            }
        }

        // Limit to max_entries
        if filtered.len() > self.config.max_entries {
            filtered.truncate(self.config.max_entries);
        }

        Ok(serde_json::json!({
            "entries": filtered,
            "count": filtered.len(),
            "query": query,
        }))
    }

    /// Tail logs (get last N lines from specified sources)
    async fn tail_logs(
        &self,
        sources: Vec<String>,
        lines: usize,
    ) -> Result<serde_json::Value, LogError> {
        let mut all_entries = Vec::new();

        for source in sources {
            let entries = self.read_log_file(&source).await?;
            all_entries.extend(entries);
        }

        // Sort by timestamp
        all_entries.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Take last N lines
        let limit = lines.min(self.config.max_entries);
        let tail_entries: Vec<LogEntry> = all_entries.into_iter().take(limit).collect();

        Ok(serde_json::json!({
            "entries": tail_entries,
            "count": tail_entries.len(),
            "lines_requested": lines,
        }))
    }

    /// Get list of available log sources
    async fn get_sources(&self) -> Result<serde_json::Value, LogError> {
        let sources = self.list_source_files().await?;

        let mut source_info = Vec::new();
        for source in &sources {
            source_info.push(self.get_source_info(source).await);
        }

        Ok(serde_json::json!({
            "sources": source_info,
            "count": sources.len(),
        }))
    }

    /// Get info about a specific source
    async fn get_source_info(&self, source: &str) -> serde_json::Value {
        let path = self.config.logs_dir.join(format!("{}.log", source));

        let (size, modified) = if path.exists() {
            match fs::metadata(&path).await {
                Ok(metadata) => {
                    let size = metadata.len();
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs());
                    (Some(size), modified)
                }
                Err(_) => (None, None),
            }
        } else {
            (None, None)
        };

        serde_json::json!({
            "name": source,
            "path": path.to_string_lossy().to_string(),
            "size": size,
            "modified_at": modified,
        })
    }

    /// Get stream info for SSE
    async fn get_stream_info(
        &self,
        sources: Vec<String>,
        filter: Option<String>,
    ) -> Result<serde_json::Value, LogError> {
        Ok(serde_json::json!({
            "status": "stream_ready",
            "sources": sources,
            "filter": filter,
            "streaming_enabled": self.config.enable_streaming,
        }))
    }

    /// List all log source files (without .log extension)
    async fn list_source_files(&self) -> Result<Vec<String>, LogError> {
        let mut sources = Vec::new();

        if !self.config.logs_dir.exists() {
            return Ok(sources);
        }

        let mut entries = fs::read_dir(&self.config.logs_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "log" {
                    if let Some(stem) = path.file_stem() {
                        if let Some(name) = stem.to_str() {
                            sources.push(name.to_string());
                        }
                    }
                }
            }
        }

        sources.sort();
        Ok(sources)
    }

    /// Read and parse a log file
    async fn read_log_file(&self, source: &str) -> Result<Vec<LogEntry>, LogError> {
        let path = self.config.logs_dir.join(format!("{}.log", source));

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let mut entries = Vec::new();

        for line in content.lines() {
            if let Some(entry) = self.parse_log_line(line, source) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }

    /// Parse a single log line
    fn parse_log_line(&self, line: &str, source: &str) -> Option<LogEntry> {
        // Try to parse as JSON first
        if let Ok(entry) = serde_json::from_str::<LogEntry>(line) {
            return Some(entry);
        }

        // Try common log formats
        // Format: [TIMESTAMP] [LEVEL] message
        if let Some(entry) = self.parse_bracketed_format(line, source) {
            return Some(entry);
        }

        // Format: TIMESTAMP LEVEL message (common logging format)
        if let Some(entry) = self.parse_space_delimited_format(line, source) {
            return Some(entry);
        }

        // Fallback: treat entire line as message with current timestamp
        Some(LogEntry {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            source: source.to_string(),
            message: line.to_string(),
            metadata: None,
        })
    }

    /// Parse bracketed format: [2024-01-15T10:30:00Z] [INFO] message
    fn parse_bracketed_format(&self, line: &str, source: &str) -> Option<LogEntry> {
        // Look for timestamp pattern
        static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                r"\[(\d{4}-\d{2}-\d{2}[T\s]\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?)\]\s*\[(\w+)\]\s*(.*)"
            ).expect("Invalid regex pattern")
        });

        if let Some(caps) = re.captures(line) {
            let timestamp_str = caps.get(1)?.as_str();
            let level = caps.get(2)?.as_str().to_uppercase();
            let message = caps.get(3)?.as_str().to_string();

            if let Ok(timestamp) = parse_timestamp(timestamp_str) {
                return Some(LogEntry {
                    timestamp,
                    level,
                    source: source.to_string(),
                    message,
                    metadata: None,
                });
            }
        }

        None
    }

    /// Parse space-delimited format: 2024-01-15 10:30:00 INFO message
    fn parse_space_delimited_format(&self, line: &str, source: &str) -> Option<LogEntry> {
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.len() >= 3 {
            // Try date time combination: 2024-01-15 10:30:00
            let datetime_str = format!("{} {}", parts[0], parts[1]);

            if let Ok(timestamp) = parse_timestamp(&datetime_str) {
                let level = parts[2].to_uppercase();
                let message = parts[3..].join(" ");

                return Some(LogEntry {
                    timestamp,
                    level,
                    source: source.to_string(),
                    message,
                    metadata: None,
                });
            }
        }

        None
    }

    /// Ensure the logs directory exists
    pub async fn ensure_logs_dir(&self) -> Result<(), LogError> {
        if !self.config.logs_dir.exists() {
            fs::create_dir_all(&self.config.logs_dir).await?;
        }
        Ok(())
    }

    /// Write a log entry (for internal use and testing)
    pub async fn write_log(&self, entry: &LogEntry) -> Result<(), LogError> {
        self.ensure_logs_dir().await?;

        let path = self.config.logs_dir.join(format!("{}.log", entry.source));
        let line = serde_json::to_string(entry)?;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        use tokio::io::AsyncWriteExt;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;

        // Broadcast to stream subscribers
        let _ = self.log_tx.send(entry.clone());

        Ok(())
    }

    /// Get current configuration
    pub fn config(&self) -> &LogServiceConfig {
        &self.config
    }
}

impl Default for LogService {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse timestamp from various formats
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, LogError> {
    // Try RFC 3339 / ISO 8601
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try RFC 2822
    if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // Try common formats
    let formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.3f",
        "%Y-%m-%dT%H:%M:%SZ",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
    ];

    for fmt in &formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }

    // If all else fails, return an error
    Err(LogError::ParseError(format!(
        "No matching timestamp format for: {}",
        s
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_bracketed_format() {
        let service = LogService::new();
        let line = "[2024-01-15T10:30:00Z] [INFO] Test message";

        let entry = service.parse_bracketed_format(line, "test");
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "Test message");
        assert_eq!(entry.source, "test");
    }

    #[test]
    fn test_parse_space_delimited_format() {
        let service = LogService::new();
        let line = "2024-01-15 10:30:00 INFO Test message";

        let entry = service.parse_space_delimited_format(line, "test");
        assert!(entry.is_some());

        let entry = entry.unwrap();
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "Test message");
    }

    #[test]
    fn test_parse_timestamp() {
        // RFC 3339
        let dt = parse_timestamp("2024-01-15T10:30:00Z").unwrap();
        assert_eq!(dt.year(), 2024);

        // Common format
        let dt = parse_timestamp("2024-01-15 10:30:00").unwrap();
        assert_eq!(dt.year(), 2024);
    }
}
