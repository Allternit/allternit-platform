//! Example: run an ACU browser-automation task on a cloud-provisioned Incus desktop.
//!
//! Usage:
//!   INCUS_URL=https://mail:8443 \
//!   INCUS_VNC_HOST=mail \
//!   INCUS_CDP_HOST=100.108.37.126 \
//!   INCUS_CLIENT_CERT=$HOME/.config/allternit/incus/client.crt \
//!   INCUS_CLIENT_KEY=$HOME/.config/allternit/incus/client.key \
//!   INCUS_INSECURE_SKIP_VERIFY=true \
//!   ACU_URL=http://localhost:8760 \
//!   ACU_TASK="go to example.com and report the title" \
//!   cargo run -p allternit-computer-cloud --example acu_browser_task
//!
//! Use INCUS_CDP_HOST as an IP address (e.g. the Tailscale IP of the Incus
//! host). The Incus HTTP proxy device rejects non-IP/non-localhost Host
//! headers, so a plain hostname here will cause CDP connection failures.
//!
//! The example provisions an Ubuntu/XFCE desktop, starts Chrome with a remote
//! debugging port inside the container, exposes that port through an Incus proxy,
//! and drives it via the ACU gateway's /v1/execute endpoint with the CDP
//! adapter pointed at that port.

use std::sync::Arc;
use std::time::Duration;

use allternit_computer_cloud::{IncusDriver, Substrate};
use allternit_driver_interface::{
    CommandSpec, EnvironmentSpec, ExecutionDriver, PolicySpec, ResourceSpec, SpawnSpec, TenantId,
};

const CDP_CONTAINER_PORT: u16 = 9222;
const CHROME_START_TIMEOUT_SECS: u64 = 60;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let incus_url = std::env::var("INCUS_URL")?;
    let vnc_host = std::env::var("INCUS_VNC_HOST").unwrap_or_else(|_| "localhost".to_string());
    let cdp_host = std::env::var("INCUS_CDP_HOST").unwrap_or_else(|_| vnc_host.clone());
    let acu_url = std::env::var("ACU_URL").unwrap_or_else(|_| "http://localhost:8760".to_string());
    let task = std::env::var("ACU_TASK")
        .unwrap_or_else(|_| "Navigate to example.com and report the page title".to_string());
    let proof_path = std::env::var("ACU_PROOF_PATH")
        .unwrap_or_else(|_| "/tmp/acu-browser-proof.png".to_string());

    let driver = Arc::new(IncusDriver::from_url(incus_url, vnc_host.clone())?);

    let tenant = TenantId::new("bot-acu-browser")?;
    let spec = SpawnSpec {
        tenant,
        project: None,
        workspace: None,
        run_id: None,
        env: EnvironmentSpec {
            spec_type: Default::default(),
            image: "allternit-desktop".to_string(),
            version: None,
            packages: vec![],
            env_vars: Default::default(),
            working_dir: None,
            mounts: vec![],
        },
        policy: PolicySpec::default_permissive(),
        resources: ResourceSpec {
            cpu_millis: 2000,
            memory_mib: 4096,
            disk_mib: Some(20_480),
            network_egress_kib: None,
            gpu_count: None,
        },
        envelope: None,
        prewarm_pool: None,
    };

    println!("Provisioning Incus desktop...");
    let handle = driver.spawn(spec).await?;
    let native_id = handle
        .driver_info
        .get("native_id")
        .cloned()
        .unwrap_or_default();
    println!("Spawned native_id: {}", native_id);

    // Incus start is asynchronous; wait for the container to reach Running.
    wait_for_running(driver.as_ref(), &native_id).await?;

    let result = run_with_desktop(
        driver.clone(),
        &handle,
        &native_id,
        &cdp_host,
        &acu_url,
        &task,
        &proof_path,
    )
    .await;

    println!("Destroying {}...", native_id);
    let _ = driver.destroy(&handle).await;
    println!("Destroyed {}", native_id);

    result
}

async fn run_with_desktop(
    driver: Arc<IncusDriver>,
    handle: &allternit_driver_interface::ExecutionHandle,
    native_id: &str,
    cdp_host: &str,
    acu_url: &str,
    task: &str,
    proof_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Wait for the desktop services to come up.
    println!("Waiting for desktop services...");
    wait_for_service(driver.as_ref(), handle, "x11vnc").await?;

    // 2. Ensure Chrome is installed in the guest.
    println!("Checking for Chrome...");
    let chrome_check = driver
        .exec(
            handle,
            CommandSpec {
                command: vec!["which".to_string(), "google-chrome".to_string()],
                env_vars: Default::default(),
                working_dir: None,
                stdin_data: None,
                capture_stdout: true,
                capture_stderr: true,
            },
        )
        .await?;

    if chrome_check.exit_code != 0 {
        println!("Chrome not found; installing Google Chrome in the guest...");
        install_chrome(driver.as_ref(), handle).await?;
    } else {
        println!("Chrome already installed.");
    }

    // 3. Start Chrome with remote debugging.
    println!(
        "Starting Chrome with remote debugging on port {}...",
        CDP_CONTAINER_PORT
    );
    write_chrome_start_script(driver.as_ref(), handle, CDP_CONTAINER_PORT).await?;
    driver
        .exec(
            handle,
            CommandSpec {
                command: vec![
                    "bash".to_string(),
                    "-c".to_string(),
                    "chmod +x /tmp/allternit-start-chrome.sh && nohup /tmp/allternit-start-chrome.sh >/tmp/chrome-cdp.log 2>&1 </dev/null &".to_string(),
                ],
                env_vars: Default::default(),
                working_dir: None,
                stdin_data: None,
                capture_stdout: true,
                capture_stderr: true,
            },
        )
        .await?;

    // Wait for Chrome to open the CDP port.
    wait_for_cdp(driver.as_ref(), handle).await?;

    // 4. Expose the CDP port on the Incus host.
    let cdp_host_port = driver
        .expose_port(native_id, "cdp", CDP_CONTAINER_PORT)
        .await?;
    let cdp_url = format!("http://{}:{}", cdp_host, cdp_host_port);
    println!("Remote CDP endpoint: {}", cdp_url);

    // 5. Drive the browser through the ACU gateway.
    let client = reqwest::Client::new();
    let session_id = uuid::Uuid::new_v4().to_string();

    // Navigate to example.com.
    let navigate_url = format!("{}/v1/execute", acu_url.trim_end_matches('/'));
    let navigate_payload = serde_json::json!({
        "action": "goto",
        "session_id": session_id,
        "run_id": uuid::Uuid::new_v4().to_string(),
        "target": "https://example.com",
        "adapter_preference": "cdp",
        "parameters": { "cdp_url": cdp_url },
    });

    println!("POST {} -> navigate", navigate_url);
    let resp = client
        .post(&navigate_url)
        .json(&navigate_payload)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.json::<serde_json::Value>().await?;
    println!(
        "navigate status={}: {}",
        status,
        serde_json::to_string_pretty(&body)?
    );
    if !status.is_success() {
        return Err(format!("navigate failed: {}", body).into());
    }

    // Extract the page title.
    let extract_payload = serde_json::json!({
        "action": "extract",
        "session_id": session_id,
        "run_id": uuid::Uuid::new_v4().to_string(),
        "adapter_preference": "cdp",
        "parameters": { "cdp_url": cdp_url, "format": "json" },
    });
    let resp = client
        .post(&navigate_url)
        .json(&extract_payload)
        .send()
        .await?;
    let body = resp.json::<serde_json::Value>().await?;
    println!("extract result: {}", serde_json::to_string_pretty(&body)?);

    // Take a screenshot.
    let screenshot_payload = serde_json::json!({
        "action": "screenshot",
        "session_id": session_id,
        "run_id": uuid::Uuid::new_v4().to_string(),
        "adapter_preference": "cdp",
        "parameters": { "cdp_url": cdp_url, "full_page": false },
    });
    let resp = client
        .post(&navigate_url)
        .json(&screenshot_payload)
        .send()
        .await?;
    let body = resp.json::<serde_json::Value>().await?;

    // Save the screenshot artifact.
    let screenshot = body
        .get("artifacts")
        .and_then(|a| a.as_array())
        .and_then(|a| a.iter().find(|x| x.get("type").and_then(|t| t.as_str()) == Some("screenshot")))
        .and_then(|x| x.get("content").or_else(|| x.get("url")))
        .or_else(|| body.get("extracted_content").and_then(|c| c.get("data_url")))
        .and_then(|u| u.as_str());
    if let Some(data_url) = screenshot {
        let b64 = data_url.split_once(',').map(|(_, b)| b).unwrap_or(data_url);
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)?;
        std::fs::write(proof_path, &bytes)?;
        println!("Screenshot saved to {} ({} bytes)", proof_path, bytes.len());
    } else {
        println!(
            "No screenshot in response: {}",
            serde_json::to_string_pretty(&body)?
        );
    }

    println!("ACU task completed: {}", task);
    Ok(())
}

async fn wait_for_running(
    driver: &IncusDriver,
    native_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Waiting for Incus instance {} to be Running...", native_id);
    for attempt in 0..60 {
        match driver.substrate().get(native_id).await {
            Ok(handle)
                if matches!(
                    handle.state,
                    allternit_computer_cloud::ComputerState::Running
                ) =>
            {
                println!("Instance running after {} seconds", attempt + 1);
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => println!("  get() attempt {}: {}", attempt + 1, e),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("timed out waiting for Incus instance to be Running".into())
}

async fn wait_for_service(
    driver: &IncusDriver,
    handle: &allternit_driver_interface::ExecutionHandle,
    process: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    for attempt in 0..30 {
        let result = driver
            .exec(
                handle,
                CommandSpec {
                    command: vec!["pgrep".to_string(), process.to_string()],
                    env_vars: Default::default(),
                    working_dir: None,
                    stdin_data: None,
                    capture_stdout: true,
                    capture_stderr: true,
                },
            )
            .await?;
        if result.exit_code == 0 {
            println!("{} ready after {} attempts", process, attempt + 1);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    Err(format!("timed out waiting for {}", process).into())
}

async fn write_chrome_start_script(
    driver: &IncusDriver,
    handle: &allternit_driver_interface::ExecutionHandle,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    use base64::Engine as _;

    let script = format!(
        r#"#!/bin/bash
export DISPLAY=:0
export DBUS_SESSION_BUS_ADDRESS=/dev/null
pkill -f 'google-chrome' || true
sleep 1
rm -rf /tmp/chrome-profile
mkdir -p /tmp/chrome-profile
cd /tmp
exec google-chrome \
  --remote-debugging-port={port} \
  --user-data-dir=/tmp/chrome-profile \
  --no-sandbox \
  --disable-dev-shm-usage \
  --disable-gpu \
  --disable-features=Translate,OptimizationHints,MediaRouter \
  --disable-component-extensions-with-background-pages \
  --disable-background-networking \
  --disable-background-timer-throttling \
  --disable-backgrounding-occluded-windows \
  --disable-breakpad \
  --disable-client-side-phishing-detection \
  --disable-component-update \
  --disable-default-apps \
  --disable-extensions \
  --disable-hang-monitor \
  --disable-ipc-flooding-protection \
  --disable-popup-blocking \
  --disable-prompt-on-repost \
  --disable-renderer-backgrounding \
  --disable-sync \
  --force-color-profile=srgb \
  --metrics-recording-only \
  --no-first-run \
  --no-default-browser-check \
  --password-store=basic \
  --use-mock-keychain \
  --enable-automation \
  --window-size=1280,720 \
  about:blank
"#,
        port = port
    );
    let b64 = base64::engine::general_purpose::STANDARD.encode(script.as_bytes());
    let command = vec![
        "bash".to_string(),
        "-c".to_string(),
        format!(
            "echo '{}' | base64 -d > /tmp/allternit-start-chrome.sh && chmod +x /tmp/allternit-start-chrome.sh",
            b64
        ),
    ];
    let result = driver
        .exec(
            handle,
            CommandSpec {
                command,
                env_vars: Default::default(),
                working_dir: None,
                stdin_data: None,
                capture_stdout: true,
                capture_stderr: true,
            },
        )
        .await?;
    if result.exit_code != 0 {
        return Err("failed to write Chrome start script".into());
    }
    Ok(())
}

async fn install_chrome(
    driver: &IncusDriver,
    handle: &allternit_driver_interface::ExecutionHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let steps = vec![
        vec!["apt-get".to_string(), "update".to_string(), "-y".to_string()],
        vec![
            "apt-get".to_string(),
            "install".to_string(),
            "-y".to_string(),
            "wget".to_string(),
            "gnupg".to_string(),
            "ca-certificates".to_string(),
        ],
        vec![
            "wget".to_string(),
            "-q".to_string(),
            "-O".to_string(),
            "/usr/share/keyrings/google-chrome.gpg".to_string(),
            "https://dl-ssl.google.com/linux/linux_signing_key.pub".to_string(),
        ],
        vec![
            "bash".to_string(),
            "-c".to_string(),
            "echo 'deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main' > /etc/apt/sources.list.d/google-chrome.list".to_string(),
        ],
        vec!["apt-get".to_string(), "update".to_string(), "-y".to_string()],
        vec!["apt-get".to_string(), "install".to_string(), "-y".to_string(), "google-chrome-stable".to_string()],
    ];

    for (i, cmd) in steps.iter().enumerate() {
        println!("  install step {}/{}: {:?}", i + 1, steps.len(), cmd[0]);
        let result = driver
            .exec(
                handle,
                CommandSpec {
                    command: cmd.clone(),
                    env_vars: Default::default(),
                    working_dir: None,
                    stdin_data: None,
                    capture_stdout: true,
                    capture_stderr: true,
                },
            )
            .await?;
        if result.exit_code != 0 {
            return Err(format!(
                "Chrome install step {} failed with exit {}",
                i + 1,
                result.exit_code
            )
            .into());
        }
    }
    Ok(())
}

async fn wait_for_cdp(
    driver: &IncusDriver,
    handle: &allternit_driver_interface::ExecutionHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Waiting for Chrome CDP port {}...", CDP_CONTAINER_PORT);
    for attempt in 0..CHROME_START_TIMEOUT_SECS {
        let result = driver
            .exec(
                handle,
                CommandSpec {
                    command: vec![
                        "bash".to_string(),
                        "-c".to_string(),
                        format!(
                            "curl -s http://127.0.0.1:{}/json/version >/dev/null 2>&1",
                            CDP_CONTAINER_PORT
                        ),
                    ],
                    env_vars: Default::default(),
                    working_dir: None,
                    stdin_data: None,
                    capture_stdout: true,
                    capture_stderr: true,
                },
            )
            .await?;
        if result.exit_code == 0 {
            println!("CDP ready after {} seconds", attempt + 1);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("timed out waiting for Chrome CDP port".into())
}
