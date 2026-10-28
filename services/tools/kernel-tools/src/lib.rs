pub mod fs;
pub mod gui;
pub mod search;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub text: String,
    pub data: Option<serde_json::Value>,
}
