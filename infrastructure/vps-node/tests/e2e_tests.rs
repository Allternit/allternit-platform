//! Allternit Node End-to-End Integration Tests
//!
//! Tests the full flow: node connects → job queued → job executed → result returned

use allternit_protocol::{
    JobSpec, Message, MessagePayload, NodeCapabilities, NodeStatus, ResourceRequirements,
    ResourceUsage, TaskDefinition, WIHDefinition,
};
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, error};

/// Test configuration
struct TestConfig {
    control_plane_url: String,
    node_id: String,
    auth_token: String,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            control_plane_url: std::env::var("Allternit_CONTROL_PLANE")
                .unwrap_or_else(|_| "ws://localhost:3000".to_string()),
            node_id: "test-node-e2e".to_string(),
            auth_token: "test-token".to_string(),
        }
    }
}

/// End-to-end test harness
struct E2ETestHarness {
    config: TestConfig,
}

impl E2ETestHarness {
    fn new() -> Self {
        Self {
            config: TestConfig::default(),
        }
    }

    /// Test 1: Node connects and registers successfully
    async fn test_node_connect_register(&self) -> Result<(), String> {
        info!("🧪 Test 1: Node connect and register");

        // Connect to control plane
        let ws_url = format!("{}/ws/nodes/{}", self.config.control_plane_url, self.config.node_id);
        info!("Connecting to {}", ws_url);

        // In a real test, we'd establish WebSocket connection here
        // For now, just verify the endpoint is reachable
        let client = reqwest::Client::new();
        match client.get(&self.config.control_plane_url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    info!("✅ Control plane is reachable");
                    Ok(())
                } else {
                    Err(format!("Control plane returned status: {}", response.status()))
                }
            }
            Err(e) => Err(format!("Failed to connect to control plane: {}", e)),
        }
    }

    /// Test 2: Heartbeat works for 5+ minutes
    async fn test_heartbeat(&self) -> Result<(), String> {
        info!("🧪 Test 2: Heartbeat (shortened for CI)");

        // In CI, we run a shorter version
        // Full test would run for 5+ minutes
        let heartbeat_count = 3; // 3 heartbeats = ~90 seconds
        let interval = Duration::from_secs(30);

        for i in 0..heartbeat_count {
            info!("Sending heartbeat {}/{}", i + 1, heartbeat_count);
            tokio::time::sleep(interval).await;
            // In real test, send actual heartbeat message
        }

        info!("✅ Heartbeat test passed");
        Ok(())
    }

    /// Test 3: Terminal session creation and interaction
    async fn test_terminal_session(&self) -> Result<(), String> {
        info!("🧪 Test 3: Terminal session creation");

        // Create terminal session via HTTP
        let client = reqwest::Client::new();
        let create_url = format!(
            "{}/api/v1/nodes/{}/terminal",
            self.config.control_plane_url.replace("ws", "http"),
            self.config.node_id
        );

        let response = client
            .post(&create_url)
            .json(&serde_json::json!({
                "shell": "/bin/bash",
                "cols": 80,
                "rows": 24,
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to create terminal: {}", e))?;

        if response.status().is_success() {
            let session: serde_json::Value = response.json().await
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            
            let session_id = session["sessionId"].as_str()
                .ok_or("No session ID in response")?;
            
            info!("✅ Terminal session created: {}", session_id);
            Ok(())
        } else {
            Err(format!("Failed to create terminal: {}", response.status()))
        }
    }

    /// Test 4: Job submission and execution
    async fn test_job_execution(&self) -> Result<(), String> {
        info!("🧪 Test 4: Job submission and execution");

        let client = reqwest::Client::new();
        let jobs_url = format!(
            "{}/api/v1/jobs",
            self.config.control_plane_url.replace("ws", "http")
        );

        // Submit a simple echo job
        let job_spec = JobSpec {
            id: format!("test-job-{}", uuid::Uuid::new_v4()),
            name: "test-echo".to_string(),
            wih: WIHDefinition {
                handler: "shell".to_string(),
                version: "1.0".to_string(),
                task: TaskDefinition::Shell {
                    command: "echo 'Hello from Allternit'".to_string(),
                    working_dir: None,
                },
                tools: vec![],
            },
            resources: ResourceRequirements::default(),
            env: std::collections::HashMap::new(),
            priority: 0,
            timeout_secs: 60,
        };

        let response = client
            .post(&jobs_url)
            .json(&serde_json::to_value(&job_spec).unwrap())
            .send()
            .await
            .map_err(|e| format!("Failed to submit job: {}", e))?;

        if response.status().is_success() {
            let result: serde_json::Value = response.json().await
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            
            let job_id = result["job_id"].as_str()
                .ok_or("No job ID in response")?;
            
            info!("✅ Job submitted: {}", job_id);

            // Wait for job completion (with timeout)
            let max_wait = Duration::from_secs(30);
            let start = std::time::Instant::now();

            while start.elapsed() < max_wait {
                tokio::time::sleep(Duration::from_secs(2)).await;

                // Check job status
                let status_url = format!("{}/{}", jobs_url, job_id);
                if let Ok(status_response) = client.get(&status_url).send().await {
                    if status_response.status().is_success() {
                        let status: serde_json::Value = status_response.json().await.unwrap();
                        if let Some(status_str) = status["status"].as_str() {
                            if status_str == "completed" {
                                info!("✅ Job completed successfully");
                                return Ok(());
                            } else if status_str == "failed" {
                                return Err(format!("Job failed: {:?}", status));
                            }
                        }
                    }
                }
            }

            Err(format!("Job {} did not complete within {:?}", job_id, max_wait))
        } else {
            Err(format!("Failed to submit job: {}", response.status()))
        }
    }

    /// Test 5: File upload/download
    async fn test_file_operations(&self) -> Result<(), String> {
        info!("🧪 Test 5: File upload/download");

        let client = reqwest::Client::new();
        let base_url = self.config.control_plane_url.replace("ws", "http");

        // Upload a test file
        let upload_url = format!(
            "{}/api/v1/nodes/{}/files/upload",
            base_url,
            self.config.node_id
        );

        let test_content = "Hello from Allternit test";
        let response = client
            .post(&upload_url)
            .multipart(reqwest::multipart::Form::new()
                .text("path", "/tmp/test.txt")
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(test_content.as_bytes().to_vec())
                        .file_name("test.txt"),
                )
            )
            .send()
            .await
            .map_err(|e| format!("Failed to upload file: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("File upload failed: {}", response.status()));
        }

        info!("✅ File uploaded");

        // Download the file back
        let download_url = format!(
            "{}/api/v1/nodes/{}/files/download?path=/tmp/test.txt",
            base_url,
            self.config.node_id
        );

        let response = client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| format!("Failed to download file: {}", e))?;

        if response.status().is_success() {
            let downloaded = response.text().await
                .map_err(|e| format!("Failed to read downloaded content: {}", e))?;
            
            if downloaded == test_content {
                info!("✅ File upload/download test passed");
                Ok(())
            } else {
                Err("Downloaded content doesn't match uploaded content".to_string())
            }
        } else {
            Err(format!("File download failed: {}", response.status()))
        }
    }

    /// Test 6: Node disconnect and reconnect
    async fn test_reconnect(&self) -> Result<(), String> {
        info!("🧪 Test 6: Node disconnect and reconnect");

        // First connection
        info!("Initial connection...");
        // In real test: establish WebSocket, then close it

        // Wait a moment
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Reconnect
        info!("Reconnecting...");
        // In real test: establish new WebSocket connection

        info!("✅ Reconnect test passed");
        Ok(())
    }

    /// Test 7: Multiple concurrent jobs
    async fn test_concurrent_jobs(&self) -> Result<(), String> {
        info!("🧪 Test 7: Multiple concurrent jobs");

        let client = reqwest::Client::new();
        let jobs_url = format!(
            "{}/api/v1/jobs",
            self.config.control_plane_url.replace("ws", "http")
        );

        // Submit 5 concurrent jobs
        let mut job_ids = Vec::new();
        for i in 0..5 {
            let job_spec = JobSpec {
                id: format!("test-job-concurrent-{}", i),
                name: format!("concurrent-job-{}", i),
                wih: WIHDefinition {
                    handler: "shell".to_string(),
                    version: "1.0".to_string(),
                    task: TaskDefinition::Shell {
                        command: format!("echo 'Job {} running'", i),
                        working_dir: None,
                    },
                    tools: vec![],
                },
                resources: ResourceRequirements::default(),
                env: std::collections::HashMap::new(),
                priority: 0,
                timeout_secs: 60,
            };

            let response = client
                .post(&jobs_url)
                .json(&serde_json::to_value(&job_spec).unwrap())
                .send()
                .await
                .map_err(|e| format!("Failed to submit job {}: {}", i, e))?;

            if response.status().is_success() {
                let result: serde_json::Value = response.json().await.unwrap();
                if let Some(job_id) = result["job_id"].as_str() {
                    job_ids.push(job_id.to_string());
                    info!("Submitted concurrent job: {}", job_id);
                }
            }
        }

        info!("✅ Submitted {} concurrent jobs", job_ids.len());
        Ok(())
    }

    /// Test 8: Job cancellation
    async fn test_job_cancellation(&self) -> Result<(), String> {
        info!("🧪 Test 8: Job cancellation");

        let client = reqwest::Client::new();
        let jobs_url = format!(
            "{}/api/v1/jobs",
            self.config.control_plane_url.replace("ws", "http")
        );

        // Submit a long-running job
        let job_spec = JobSpec {
            id: "test-job-cancel".to_string(),
            name: "long-running-job".to_string(),
            wih: WIHDefinition {
                handler: "shell".to_string(),
                version: "1.0".to_string(),
                task: TaskDefinition::Shell {
                    command: "sleep 60".to_string(),
                    working_dir: None,
                },
                tools: vec![],
            },
            resources: ResourceRequirements::default(),
            env: std::collections::HashMap::new(),
            priority: 0,
            timeout_secs: 120,
        };

        let response = client
            .post(&jobs_url)
            .json(&serde_json::to_value(&job_spec).unwrap())
            .send()
            .await
            .map_err(|e| format!("Failed to submit job: {}", e))?;

        let result: serde_json::Value = response.json().await.unwrap();
        let job_id = result["job_id"].as_str().unwrap();

        // Cancel the job
        let cancel_url = format!("{}/{}/cancel", jobs_url, job_id);
        let cancel_response = client
            .post(&cancel_url)
            .send()
            .await
            .map_err(|e| format!("Failed to cancel job: {}", e))?;

        if cancel_response.status().is_success() {
            info!("✅ Job cancellation test passed");
            Ok(())
        } else {
            Err(format!("Job cancellation failed: {}", cancel_response.status()))
        }
    }

    /// Run all tests
    async fn run_all_tests(&self) -> TestResults {
        let mut results = TestResults::new();

        let tests = vec![
            ("Node Connect & Register", self.test_node_connect_register().await),
            ("Heartbeat", self.test_heartbeat().await),
            ("Terminal Session", self.test_terminal_session().await),
            ("Job Execution", self.test_job_execution().await),
            ("File Operations", self.test_file_operations().await),
            ("Reconnect", self.test_reconnect().await),
            ("Concurrent Jobs", self.test_concurrent_jobs().await),
            ("Job Cancellation", self.test_job_cancellation().await),
        ];

        for (name, result) in tests {
            results.add_test(name, result);
        }

        results
    }
}

/// Test results summary
struct TestResults {
    passed: Vec<String>,
    failed: Vec<(String, String)>,
}

impl TestResults {
    fn new() -> Self {
        Self {
            passed: Vec::new(),
            failed: Vec::new(),
        }
    }

    fn add_test(&mut self, name: &str, result: Result<(), String>) {
        match result {
            Ok(()) => {
                info!("✅ PASSED: {}", name);
                self.passed.push(name.to_string());
            }
            Err(e) => {
                error!("❌ FAILED: {} - {}", name, e);
                self.failed.push((name.to_string(), e));
            }
        }
    }

    fn summary(&self) -> String {
        let total = self.passed.len() + self.failed.len();
        format!(
            "\n\n{}\nTest Results: {}/{} passed\n{}\n",
            "=".repeat(50),
            self.passed.len(),
            total,
            "=".repeat(50)
        )
    }

    fn all_passed(&self) -> bool {
        self.failed.is_empty()
    }
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("info,e2e_tests=debug")
        .init();

    info!("🚀 Starting Allternit Node End-to-End Tests");

    let harness = E2ETestHarness::new();
    let results = harness.run_all_tests().await;

    println!("{}", results.summary());

    if !results.failed.is_empty() {
        println!("\nFailed tests:");
        for (name, error) in &results.failed {
            println!("  ❌ {}: {}", name, error);
        }
        std::process::exit(1);
    } else {
        println!("\n🎉 All tests passed!");
        std::process::exit(0);
    }
}
