# Allternit VM Execution System

Complete virtual machine execution environment for Allternit, providing sandboxed Linux environments on macOS using Apple Virtualization.framework.

## Overview

The Allternit VM system consists of multiple components working together to provide secure, isolated execution environments:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Allternit VM Architecture                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────────────────────┐  │
│  │  CLI Tool   │    │ Desktop App │    │         allternit-vm-executor         │  │
│  │  (allternit vm)   │◄──►│  (Electron) │◄──►│       (runs inside VM)          │  │
│  └─────────────┘    └──────┬──────┘    └─────────────────────────────────┘  │
│                            │                          │                      │
│                            ▼                          ▼                      │
│                   ┌─────────────────┐        ┌──────────────┐               │
│                   │  Swift VM Manager│        │   VSOCK      │               │
│                   │  (Virtualization)│◄──────►│  (port 8080) │               │
│                   └─────────────────┘        └──────────────┘               │
│                            │                                                 │
│                            ▼                                                 │
│                   ┌─────────────────┐                                       │
│                   │   Linux Kernel  │                                       │
│                   │   Ubuntu 22.04  │                                       │
│                   └─────────────────┘                                       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Components

### 1. VM Executor (Rust)
**Location:** `1-kernel/execution/allternit-vm-executor/`

The guest agent that runs inside the Linux VM:
- Listens on VSOCK port 8080 for commands
- Executes commands in bubblewrap sandboxes
- Returns stdout/stderr/exit codes to host

```bash
# Build
cargo build --release -p allternit-vm-executor

# Run (inside VM)
./allternit-vm-executor
```

### 2. VM Image Builder (Rust)
**Location:** `1-kernel/execution/allternit-vm-image-builder/`

Downloads or builds VM images:

```bash
# Download pre-built images
allternit-vm-image-builder download --version 1.1.0

# Build locally (Linux only)
allternit-vm-image-builder build --ubuntu-version 22.04
```

**Image Locations:**
```
~/.allternit/vm-images/
├── vmlinux-6.5.0-allternit              # Linux kernel
├── initrd.img-6.5.0-allternit           # Initial ramdisk
├── ubuntu-22.04-allternit-v1.1.0.ext4   # Root filesystem
└── version.json                   # Metadata
```

### 3. Swift VM Manager (macOS)
**Location:** `7-apps/shell/desktop/native/vm-manager/`

Native macOS VM management using Virtualization.framework:

```bash
# Build
cd 7-apps/shell/desktop/native/vm-manager
swift build -c release

# CLI Usage
./.build/release/vm-manager-cli start
./.build/release/vm-manager-cli stop
./.build/release/vm-manager-cli status
./.build/release/vm-manager-cli exec "uname -a"
```

### 4. Node.js Bridge
**Location:** `7-apps/shell/desktop/native/vm-manager-node/`

TypeScript wrapper for the Swift CLI:

```typescript
import { AllternitVMManager } from '@allternit/vm-manager';

const manager = new AllternitVMManager();
await manager.start();
const result = await manager.execute({
  command: "npm",
  args: ["test"]
});
await manager.stop();
```

### 5. Desktop UI Components
**Location:** `7-apps/shell/desktop/src-electron/renderer/components/vm/`

React components for VM management:
- `VMManagerPanel` - Main management UI
- `VMStatus` - Status display
- `VMControls` - Start/stop buttons
- `VMCommandExecutor` - Command interface
- `VMSetupWizard` - Initial setup

## Quick Start

### Prerequisites

- macOS 13.0+ (for Virtualization.framework)
- Xcode Command Line Tools
- Rust toolchain
- Swift 5.9+

### 1. Download VM Images

```bash
# Using the image builder
cargo run --release -p allternit-vm-image-builder -- download --version 1.1.0

# Or manually from GitHub Releases
curl -L -o ubuntu-22.04-allternit-v1.1.0.ext4.zst \
  https://github.com/Allternit/allternit-platform/releases/download/v1.1.0/ubuntu-22.04-allternit-v1.1.0.ext4.zst
zstd -d ubuntu-22.04-allternit-v1.1.0.ext4.zst
```

### 2. Build VM Manager

```bash
cd 7-apps/shell/desktop/native/vm-manager
swift build -c release
```

### 3. Start VM

```bash
./7-apps/shell/desktop/native/vm-manager/.build/release/vm-manager-cli start
```

### 4. Execute Commands

```bash
# Using CLI
./vm-manager-cli exec "ls -la"

# Or via desktop app
# Open Allternit Desktop → VM tab → Command Executor
```

## Configuration

### VM Configuration Options

```typescript
interface VMConfiguration {
  vmName?: string;           // Default: "allternit-vm"
  kernelPath?: string;       // Path to vmlinux
  initrdPath?: string;       // Path to initrd
  rootfsPath?: string;       // Path to ext4 rootfs
  cpuCount?: number;         // Default: 4
  memorySizeMB?: number;     // Default: 4096
  vsockPort?: number;        // Default: 8080
  socketPath?: string;       // Unix socket for CLI
}
```

### Environment Variables

```bash
export Allternit_VM_KERNEL="$HOME/.allternit/vm-images/vmlinux-6.5.0-allternit"
export Allternit_VM_INITRD="$HOME/.allternit/vm-images/initrd.img-6.5.0-allternit"
export Allternit_VM_ROOTFS="$HOME/.allternit/vm-images/ubuntu-22.04-allternit-v1.1.0.ext4"
export Allternit_VM_SOCKET="$HOME/.allternit/desktop-vm.sock"
```

## Architecture Details

### VSOCK Communication Protocol

The host and VM communicate over VSOCK (virtual socket):

```
┌──────────────┐                    ┌──────────────┐
│     Host     │  VSOCK Port 8080   │      VM      │
│              │◄──────────────────►│              │
│  vm-manager  │     JSON RPC       │  allternit-vm-     │
│    -cli      │                    │   executor   │
└──────────────┘                    └──────────────┘
```

**Message Format:**
```json
// Request
{
  "id": "req-1",
  "type": "execute",
  "command": "npm",
  "args": ["install"],
  "workingDir": "/workspace",
  "timeout": 60000
}

// Response
{
  "id": "req-1",
  "success": true,
  "stdout": "added 42 packages...",
  "stderr": "",
  "exitCode": 0,
  "executionTime": 5234
}
```

### VM Lifecycle States

```
stopped → starting → running → stopping → stopped
            ↓              ↓
         error          paused
```

## IPC Interface

### From Renderer Process

```typescript
// Get status
const status = await window.electronAPI.vm.getStatus();

// Start VM
await window.electronAPI.vm.start();

// Stop VM
await window.electronAPI.vm.stop();

// Execute command
const result = await window.electronAPI.vm.execute({
  command: "python3",
  args: ["script.py"],
  timeoutMs: 30000
});

// Listen for status changes
window.electronAPI.vm.onStatusChanged((state) => {
  console.log("VM state:", state);
});
```

## Building from Source

### 1. Build VM Executor (for Linux)

```bash
cargo build --release -p allternit-vm-image-builder
```

### 2. Build Swift VM Manager (macOS)

```bash
cd 7-apps/shell/desktop/native/vm-manager
swift build -c release
```

### 3. Build Node.js Bridge

```bash
cd 7-apps/shell/desktop/native/vm-manager-node
npm install
npm run build
```

### 4. Build Desktop App

```bash
cd 7-apps/shell/desktop
npm install
npm run build
```

## Testing

### Unit Tests

```bash
# Rust components
cargo test -p allternit-vm-executor
cargo test -p allternit-vm-image-builder

# Swift components
cd native/vm-manager
swift test

# Node.js bridge
cd native/vm-manager-node
npm test
```

### Integration Test

```bash
# Start VM and run test suite
./scripts/test-vm.sh
```

## Troubleshooting

### VM Won't Start

1. Check if images exist:
   ```bash
   ls -la ~/.allternit/vm-images/
   ```

2. Check system requirements:
   ```bash
   # macOS version
   sw_vers -productVersion  # Should be 13.0+
   ```

3. Check VM logs:
   ```bash
   ./vm-manager-cli start 2>&1 | tee vm.log
   ```

### Command Execution Fails

1. Verify VM is running:
   ```bash
   ./vm-manager-cli status
   ```

2. Check VSOCK connection:
   ```bash
   # In VM (via serial console)
   ss -ln | grep 8080
   ```

3. Check executor logs:
   ```bash
   # In VM
   journalctl -u allternit-vm-executor
   ```

### Image Download Issues

1. Check GitHub release exists:
   ```bash
   curl -I https://github.com/Allternit/allternit-platform/releases/download/v1.1.0/ubuntu-22.04-allternit-v1.1.0.ext4.zst
   ```

2. Verify disk space:
   ```bash
   df -h ~
   ```

## Security

### Sandboxing

- Commands run in bubblewrap sandboxes inside the VM
- Network access controlled by VM configuration
- File system access restricted to /workspace

### Resource Limits

Default limits per execution:
- CPU: Shares of VM CPU
- Memory: 2GB per sandbox
- Timeout: 60 seconds (configurable)

## Release Images

Pre-built VM images are available on GitHub Releases:

| File | Size | Description |
|------|------|-------------|
| `vmlinux-6.5.0-allternit` | 11 MB | Linux kernel |
| `initrd.img-6.5.0-allternit` | 32 MB | Initial ramdisk |
| `ubuntu-22.04-allternit-v1.1.0.ext4.zst` | 21 MB | Compressed rootfs |
| `version-1.1.0-amd64.json` | <1 KB | Metadata & checksums |

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for development guidelines.

## License

MIT License - See [LICENSE](../../LICENSE)
