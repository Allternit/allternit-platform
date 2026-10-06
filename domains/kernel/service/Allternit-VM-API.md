# Allternit VM API Reference

Complete API reference for the Allternit VM execution system.

## Table of Contents

- [Rust API](#rust-api)
- [Swift API](#swift-api)
- [TypeScript/JavaScript API](#typescriptjavascript-api)
- [IPC Protocol](#ipc-protocol)
- [VSOCK Protocol](#vsock-protocol)

---

## Rust API

### allternit-vm-executor

Guest agent that runs inside the VM.

#### Command Line

```bash
allternit-vm-executor [OPTIONS]

Options:
    -c, --config <FILE>     Configuration file path
    -p, --port <PORT>       VSOCK port (default: 8080)
    -l, --log-level <LEVEL> Log level (error|warn|info|debug|trace)
    -h, --help              Print help
    -V, --version           Print version
```

#### Configuration File

```toml
# /etc/allternit/vm-executor.toml
vsock_port = 8080
log_level = "info"
max_sessions = 50
workspace_path = "/workspace"

[sandbox]
use_bubblewrap = true
network = "host"
max_memory_mb = 2048
max_cpu_percent = 100
```

### allternit-vm-image-builder

Downloads or builds VM images.

#### Command Line

```bash
# Download pre-built images
allternit-vm-image-builder download [OPTIONS]
    --version <VER>    Image version (default: 1.1.0)
    --no-verify        Skip checksum verification

# Build locally (Linux only)
allternit-vm-image-builder build [OPTIONS]
    --ubuntu-version <VER>  Ubuntu version (default: 22.04)
    --packages <PKGS>       Additional packages to install
    --no-toolchains         Skip toolchain installation

# Check for updates
allternit-vm-image-builder check-update

# Verify existing images
allternit-vm-image-builder verify

# Clean up images
allternit-vm-image-builder clean
```

#### Library API

```rust
use allternit_vm_image_builder::{ImageDownloader, ImageConfig, LocalBuilder};

// Download images
let downloader = ImageDownloader::new(
    "Allternit/allternit-platform",
    "1.1.0",
    Path::new("~/.allternit/vm-images")
);
downloader.download(true).await?;

// Build images locally (Linux only)
let config = ImageConfig {
    ubuntu_version: "22.04".to_string(),
    additional_packages: vec!["nodejs".to_string()],
    include_toolchains: true,
    output_dir: PathBuf::from("~/.allternit/vm-images"),
};
let builder = LocalBuilder::new(config);
builder.build().await?;
```

---

## Swift API

### AllternitVMManager

Main class for managing VMs on macOS.

```swift
import AllternitVMManager

// Configuration
let config = AllternitVMConfiguration(
    vmName: "allternit-vm",
    kernelPath: "/Users/.../.allternit/vm-images/vmlinux-6.5.0-allternit",
    initrdPath: "/Users/.../.allternit/vm-images/initrd.img-6.5.0-allternit",
    rootfsPath: "/Users/.../.allternit/vm-images/ubuntu-22.04-allternit-v1.1.0.ext4",
    cpuCount: 4,
    memorySize: 4 * 1024 * 1024 * 1024,  // 4 GB
    vsockPort: 8080,
    socketPath: "/Users/.../.allternit/desktop-vm.sock"
)

// Create manager
let manager = VMManager(configuration: config)

// Status updates
manager.onStatusChange { status in
    print("VM state: \(status.state)")
}

// Lifecycle
Task {
    try await manager.start()
    
    // Execute command
    let result = try await manager.sendCommand("uname -a")
    print(result.stdout)
    
    try await manager.stop()
}
```

### VMConfiguration

```swift
public struct AllternitVMConfiguration: Codable, Sendable {
    public let vmName: String
    public let kernelPath: String
    public let initrdPath: String
    public let rootfsPath: String
    public let cpuCount: Int
    public let memorySize: UInt64
    public let vsockPort: UInt32
    public let socketPath: String
}
```

### VMStatus

```swift
public struct VMStatus: Codable, Sendable {
    public let state: VMState
    public let vmName: String
    public let pid: Int?
    public let socketPath: String
    public let vsockPort: UInt32
    public let errorMessage: String?
    public let uptime: TimeInterval?
}

public enum VMState: String, Codable, Sendable {
    case stopped
    case starting
    case running
    case stopping
    case error
    case paused
}
```

### CommandResult

```swift
public struct CommandResult: Codable, Sendable {
    public let success: Bool
    public let stdout: String
    public let stderr: String
    public let exitCode: Int32
    public let executionTime: TimeInterval
}
```

### VSOCKChannel

Low-level VSOCK communication.

```swift
let channel = VSOCKChannel(port: 8080)

// Connect to VM
await channel.connect(connection)

// Send request
let request = VSOCKRequest(
    type: .execute,
    command: "npm",
    args: ["install"],
    timeout: 60000
)
let response = try await channel.sendRequest(request)
```

---

## TypeScript/JavaScript API

### AllternitVMManager

```typescript
import { AllternitVMManager } from '@allternit/vm-manager';

// Create manager
const manager = new AllternitVMManager({
  vmName: 'allternit-vm',
  cpuCount: 4,
  memorySizeMB: 4096,
});

// Status events
manager.on('statusChanged', (status: VMStatus) => {
  console.log('State:', status.state);
});

// Lifecycle
await manager.start();
await manager.restart();
await manager.stop();

// Execute command
const result = await manager.execute('npm', ['test'], {
  workingDir: '/workspace/project',
  timeout: 60000
});

console.log(result.stdout);
console.log(result.exitCode);
```

### Interfaces

```typescript
interface VMConfiguration {
  vmName?: string;
  kernelPath?: string;
  initrdPath?: string;
  rootfsPath?: string;
  cpuCount?: number;
  memorySizeMB?: number;
  vsockPort?: number;
  socketPath?: string;
}

interface VMStatus {
  state: VMState;
  vmName: string;
  pid?: number;
  socketPath: string;
  vsockPort: number;
  errorMessage?: string;
  uptime?: number;
}

type VMState = 'stopped' | 'starting' | 'running' | 'stopping' | 'error' | 'paused';

interface CommandResult {
  success: boolean;
  stdout: string;
  stderr: string;
  exitCode: number;
  executionTime: number;
}

interface ExecuteOptions {
  workingDir?: string;
  env?: Record<string, string>;
  timeout?: number;
}
```

### React Hook

```typescript
import { useVM } from './hooks/useVM';

function MyComponent() {
  const {
    status,
    isRunning,
    isStarting,
    error,
    startVM,
    stopVM,
    executeCommand,
    checkImages,
    downloadImages
  } = useVM();

  const runCommand = async () => {
    const result = await executeCommand({
      command: 'cargo',
      args: ['build'],
      workingDir: '/workspace'
    });
    console.log(result);
  };

  return (
    <div>
      <p>Status: {status?.state}</p>
      <button onClick={startVM} disabled={isRunning}>
        Start VM
      </button>
    </div>
  );
}
```

---

## IPC Protocol

### Channels

#### Main → Renderer (Invoke)

```typescript
// Get VM status
const status = await window.electronAPI.vm.getStatus();

// Control VM
await window.electronAPI.vm.start();
await window.electronAPI.vm.stop();
await window.electronAPI.vm.restart();

// Execute command
const result = await window.electronAPI.vm.execute({
  command: string,
  args?: string[],
  workingDir?: string,
  env?: Record<string, string>,
  timeoutMs?: number
});

// Image management
const exists = await window.electronAPI.vm.checkImages();
await window.electronAPI.vm.downloadImages({ version?: string });
await window.electronAPI.vm.setup({ force?: boolean, version?: string });
```

#### Renderer → Main (Events)

```typescript
// Listen for status changes
const unsubscribe = window.electronAPI.vm.onStatusChanged((state: VMState) => {
  console.log('VM state changed:', state);
});

// Cleanup
unsubscribe();
```

### Message Types

```typescript
// IPC Request
type VMRequest =
  | { type: 'getStatus' }
  | { type: 'start' }
  | { type: 'stop' }
  | { type: 'restart' }
  | { type: 'execute'; options: VMExecuteOptions }
  | { type: 'checkImages' }
  | { type: 'downloadImages'; options?: VMSetupOptions }
  | { type: 'setup'; options?: VMSetupOptions };

// IPC Response
type VMResponse =
  | { type: 'status'; status: VMInfo }
  | { type: 'result'; result: VMExecuteResult }
  | { type: 'boolean'; value: boolean }
  | { type: 'error'; error: string };

// Status Event
type VMStatusEvent = {
  state: VMState;
  timestamp: number;
};
```

---

## VSOCK Protocol

### Connection

```
Host (macOS)                    Guest (Linux)
┌─────────────────┐             ┌─────────────────┐
│  VSOCK Port     │◄───────────►│  VSOCK Port     │
│  (any)          │  Connect    │  8080           │
└────────┬────────┘             └────────┬────────┘
         │                               │
         ▼                               ▼
┌─────────────────┐             ┌─────────────────┐
│  vm-manager-cli │             │  allternit-vm-executor│
└─────────────────┘             └─────────────────┘
```

### Message Format

All messages are length-prefixed JSON:

```
┌──────────────┬─────────────────────────────────────┐
│  Length (4)  │  JSON Payload (variable)            │
│  Big-endian  │  UTF-8 encoded                      │
└──────────────┴─────────────────────────────────────┘
```

### Request Types

#### Execute Command

```json
{
  "id": "req-123",
  "type": "execute",
  "command": "npm",
  "args": ["install", "--save", "lodash"],
  "workingDir": "/workspace/project",
  "env": {
    "NODE_ENV": "production",
    "DEBUG": "1"
  },
  "timeout": 60000
}
```

#### Get Status

```json
{
  "id": "req-124",
  "type": "status"
}
```

#### Ping

```json
{
  "id": "req-125",
  "type": "ping"
}
```

### Response Format

```json
{
  "id": "req-123",
  "success": true,
  "stdout": "added 42 packages...",
  "stderr": "npm WARN deprecated...",
  "exitCode": 0,
  "executionTime": 5234
}
```

### Error Response

```json
{
  "id": "req-123",
  "success": false,
  "error": "Command timeout after 60000ms",
  "exitCode": -1,
  "executionTime": 60000
}
```

### TypeScript Definitions

```typescript
// VSOCK Request
interface VSOCKRequest {
  id: string;
  type: 'execute' | 'status' | 'ping';
  command?: string;
  args?: string[];
  workingDir?: string;
  env?: Record<string, string>;
  timeout?: number;
}

// VSOCK Response
interface VSOCKResponse {
  id: string;
  success: boolean;
  stdout?: string;
  stderr?: string;
  exitCode?: number;
  executionTime?: number;
  error?: string;
}
```

---

## Error Codes

### VM Errors

| Error | Description |
|-------|-------------|
| `VMError.kernelNotFound` | Kernel image not found |
| `VMError.initrdNotFound` | Initrd image not found |
| `VMError.rootfsNotFound` | Root filesystem not found |
| `VMError.vmAlreadyRunning` | VM is already running |
| `VMError.vmNotRunning` | VM is not running |
| `VMError.vsockConnectionFailed` | Could not connect via VSOCK |
| `VMError.commandExecutionFailed` | Command execution failed |

### VSOCK Errors

| Error | Description |
|-------|-------------|
| `VSOCKError.notConnected` | Not connected to VM |
| `VSOCKError.writeFailed` | Failed to write to socket |
| `VSOCKError.timeout` | Request timeout |
| `VSOCKError.invalidResponse` | Invalid response received |

---

## Examples

### Full Workflow (Rust)

```rust
use allternit_vm_image_builder::ImageDownloader;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Download images
    let downloader = ImageDownloader::new(
        "Allternit/allternit-platform",
        "1.1.0",
        Path::new("~/.allternit/vm-images")
    );
    
    if !downloader.check_existing().await? {
        println!("Downloading VM images...");
        downloader.download(true).await?;
    }
    
    // 2. Start VM (via Swift CLI or native integration)
    // ...
    
    Ok(())
}
```

### Full Workflow (TypeScript)

```typescript
import { AllternitVMManager } from '@allternit/vm-manager';

async function main() {
  // 1. Create manager
  const manager = new AllternitVMManager();
  
  // 2. Check/download images
  const imagesExist = await manager.checkImages();
  if (!imagesExist) {
    console.log('Downloading VM images...');
    await manager.downloadImages({ version: '1.1.0' });
  }
  
  // 3. Start VM
  console.log('Starting VM...');
  await manager.start();
  
  // 4. Execute commands
  const result = await manager.execute('node', ['--version']);
  console.log('Node version:', result.stdout.trim());
  
  // 5. Stop VM
  await manager.stop();
}

main().catch(console.error);
```

---

## See Also

- [Allternit VM README](./Allternit-VM-README.md) - Overview and quick start
- [Architecture Documentation](../../docs/architecture/vm-execution.md)
- [Troubleshooting Guide](../../docs/troubleshooting/vm-issues.md)
