# Building Allternit VM Images

Complete guide for building VM images from scratch.

## Overview

This document explains how to build the Linux VM images used by Allternit. Due to platform limitations, images **must be built on Linux** (macOS cannot create ext4 filesystems).

## Prerequisites

### Build Machine Requirements

- Ubuntu 22.04 LTS (or compatible Linux distribution)
- 20 GB free disk space
- Root access (for mounting filesystems)
- Internet connection

### Required Packages

```bash
sudo apt-get update
sudo apt-get install -y \
    debootstrap \
    qemu-utils \
    e2fsprogs \
    zstd \
    wget \
    curl \
    build-essential
```

## Image Components

### 1. Linux Kernel (vmlinux)

The uncompressed kernel image for Firecracker/microVMs.

**Source:** Ubuntu cloud images

```bash
wget https://cloud-images.ubuntu.com/releases/22.04/release/unpacked/ubuntu-22.04-server-cloudimg-amd64-vmlinuz-generic \
    -O vmlinux-6.5.0-allternit
```

### 2. Initial Ramdisk (initrd)

Boot-time filesystem and drivers.

**Source:** Ubuntu cloud images

```bash
wget https://cloud-images.ubuntu.com/releases/22.04/release/unpacked/ubuntu-22.04-server-cloudimg-amd64-initrd-generic \
    -O initrd.img-6.5.0-allternit
```

### 3. Root Filesystem (rootfs)

The main ext4 filesystem containing:
- Ubuntu 22.04 base system
- allternit-vm-executor (guest agent)
- Node.js, Python, Rust toolchains
- Git, curl, and other utilities
- Bubblewrap for sandboxing

## Building the Root Filesystem

### Method 1: Using allternit-vm-image-builder (Recommended)

```bash
# Clone repository
git clone https://github.com/Allternit/allternit-platform.git
cd allternit

# Build the image builder
cargo build --release -p allternit-vm-image-builder

# Build images
./target/release/allternit-vm-image-builder build \
    --ubuntu-version 22.04 \
    --packages "nodejs,npm,python3,python3-pip"
```

### Method 2: Manual Build

#### Step 1: Create Disk Image

```bash
# Create 2GB empty image
dd if=/dev/zero of=ubuntu-22.04-allternit-v1.1.0.ext4 bs=1M count=2048

# Create ext4 filesystem
mkfs.ext4 ubuntu-22.04-allternit-v1.1.0.ext4

# Create mount point
sudo mkdir -p /mnt/rootfs

# Mount
sudo mount -o loop ubuntu-22.04-allternit-v1.1.0.ext4 /mnt/rootfs
```

#### Step 2: Install Base System

```bash
# Download Ubuntu base
cd /tmp
wget http://cdimage.ubuntu.com/ubuntu-base/releases/22.04/release/ubuntu-base-22.04-base-amd64.tar.gz

# Extract to image
sudo tar -xzf ubuntu-base-22.04-base-amd64.tar.gz -C /mnt/rootfs

# Mount special filesystems
sudo mount --bind /dev /mnt/rootfs/dev
sudo mount --bind /proc /mnt/rootfs/proc
sudo mount --bind /sys /mnt/rootfs/sys
```

#### Step 3: Configure System

```bash
# Enter chroot
sudo chroot /mnt/rootfs /bin/bash

# Set up DNS
echo "nameserver 8.8.8.8" > /etc/resolv.conf

# Update package lists
apt-get update

# Install essential packages
apt-get install -y \
    systemd \
    systemd-sysv \
    openssh-server \
    iproute2 \
    iputils-ping \
    curl \
    wget \
    ca-certificates \
    nano \
    vim \
    htop \
    git \
    bubblewrap \
    zstd \
    nodejs \
    npm \
    python3 \
    python3-pip

# Install Node.js 20.x
curl -fsSL https://deb.nodesource.com/setup_20.x | bash -
apt-get install -y nodejs

# Clean up
apt-get clean
rm -rf /var/lib/apt/lists/*

# Exit chroot
exit
```

#### Step 4: Install allternit-vm-executor

```bash
# Build executor (on build machine)
cd /path/to/allternit
cargo build --release -p allternit-vm-executor --target x86_64-unknown-linux-musl

# Copy to image
sudo cp target/x86_64-unknown-linux-musl/release/allternit-vm-executor \
    /mnt/rootfs/usr/bin/
sudo chmod +x /mnt/rootfs/usr/bin/allternit-vm-executor

# Create config directory
sudo mkdir -p /mnt/rootfs/etc/allternit

# Create config file
sudo tee /mnt/rootfs/etc/allternit/vm-executor.toml << 'EOF'
vsock_port = 8080
log_level = "info"
max_sessions = 50
workspace_path = "/workspace"

[sandbox]
use_bubblewrap = true
network = "host"
max_memory_mb = 2048
EOF

# Create systemd service
sudo tee /mnt/rootfs/etc/systemd/system/allternit-vm-executor.service << 'EOF'
[Unit]
Description=Allternit VM Executor
After=network.target

[Service]
Type=simple
ExecStart=/usr/bin/allternit-vm-executor
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# Enable service
sudo ln -sf /etc/systemd/system/allternit-vm-executor.service \
    /mnt/rootfs/etc/systemd/system/multi-user.target.wants/allternit-vm-executor.service
```

#### Step 5: Finalize

```bash
# Unmount filesystems
sudo umount /mnt/rootfs/dev
sudo umount /mnt/rootfs/proc
sudo umount /mnt/rootfs/sys
sudo umount /mnt/rootfs

# Compress image
zstd -19 -T0 ubuntu-22.04-allternit-v1.1.0.ext4 \
    -o ubuntu-22.04-allternit-v1.1.0.ext4.zst

# Generate checksums
sha256sum ubuntu-22.04-allternit-v1.1.0.ext4 > checksums.txt
sha256sum vmlinux-6.5.0-allternit >> checksums.txt
sha256sum initrd.img-6.5.0-allternit >> checksums.txt
```

## CI/CD Build

### GitHub Actions Workflow

```yaml
name: Build VM Images

on:
  workflow_dispatch:
    inputs:
      version:
        description: 'Image version'
        required: true
        default: '1.1.0'

jobs:
  build:
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
      
      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y debootstrap qemu-utils e2fsprogs zstd
      
      - name: Build images
        run: |
          cargo build --release -p allternit-vm-executor --target x86_64-unknown-linux-musl
          cargo run --release -p allternit-vm-image-builder -- build
      
      - name: Upload to release
        uses: softprops/action-gh-release@v1
        with:
          tag_name: v${{ github.event.inputs.version }}
          files: vm-images/*
```

## Docker Build (Alternative)

```dockerfile
FROM ubuntu:22.04

RUN apt-get update && apt-get install -y \
    debootstrap \
    e2fsprogs \
    zstd \
    curl

COPY build-images.sh /build/
RUN chmod +x /build/build-images.sh

CMD ["/build/build-images.sh"]
```

```bash
# Build
docker build -t allternit-vm-builder .

# Run
docker run --rm --privileged \
    -v $(pwd)/output:/output \
    allternit-vm-builder
```

## Image Specifications

### Filesystem Layout

```
/
├── bin/                    # Essential binaries
├── etc/
│   └── allternit/
│       └── vm-executor.toml    # Allternit configuration
├── home/
│   └── allternit/               # User home
├── opt/                   # Optional packages
├── usr/
│   ├── bin/
│   │   └── allternit-vm-executor   # Guest agent
│   └── local/             # Local installations
├── var/
│   └── log/               # Log files
└── workspace/             # Shared workspace (mounted)
```

### Installed Packages

**Core:**
- systemd
- openssh-server
- iproute2
- curl, wget
- ca-certificates

**Development:**
- git
- nodejs (20.x)
- npm
- python3
- python3-pip
- bubblewrap

**Utilities:**
- nano, vim
- htop
- zstd

### Default Configuration

**VM Resources:**
- CPU: 4 cores
- Memory: 4 GB
- Disk: 2 GB rootfs

**Network:**
- NAT via Virtualization.framework
- No exposed ports (VSOCK only)

**Security:**
- No root password
- Key-based SSH (optional)
- Bubblewrap sandboxing

## Troubleshooting

### Build Fails

**debootstrap errors:**
```bash
# Check internet connection
ping archive.ubuntu.com

# Try different mirror
sudo debootstrap --mirror http://mirror.math.princeton.edu/pub/ubuntu jammy /mnt/rootfs
```

**Out of disk space:**
```bash
# Check space
df -h

# Clean up
sudo apt-get clean
sudo rm -rf /var/cache/apt/archives/*
```

### Image Won't Boot

**Check kernel/initrd compatibility:**
```bash
# Verify kernel version
file vmlinux-6.5.0-allternit

# Check initrd format
file initrd.img-6.5.0-allternit
lsinitramfs initrd.img-6.5.0-allternit | head
```

**Mount and inspect:**
```bash
# Mount rootfs
sudo mount -o loop ubuntu-22.04-allternit-v1.1.0.ext4 /mnt/inspect

# Check systemd
ls /mnt/inspect/lib/systemd/systemd

# Verify executor
file /mnt/inspect/usr/bin/allternit-vm-executor

# Unmount
sudo umount /mnt/inspect
```

### VSOCK Connection Issues

**Check executor is running:**
```bash
# In VM (via serial console)
systemctl status allternit-vm-executor
ss -ln | grep 8080
```

**Test connection:**
```bash
# From host
nc -U /path/to/vsock/socket
```

## Optimization

### Reduce Image Size

```bash
# Remove unnecessary packages
sudo chroot /mnt/rootfs apt-get remove -y \
    snapd \
    landscape-client \
    ubuntu-release-upgrader-core

# Clean up
sudo chroot /mnt/rootfs apt-get autoremove -y
sudo chroot /mnt/rootfs apt-get clean
sudo rm -rf /mnt/rootfs/var/lib/apt/lists/*

# Zero free space for better compression
dd if=/dev/zero of=/mnt/rootfs/zero bs=1M || true
rm /mnt/rootfs/zero
```

### Faster Compression

```bash
# Use multiple threads
zstd -T0 -19 ubuntu-22.04-allternit-v1.1.0.ext4

# Trade compression for speed
zstd -T0 -3 ubuntu-22.04-allternit-v1.1.0.ext4
```

## Verification

### Checksum Verification

```bash
# Create checksum file
cat > version-1.1.0-amd64.json << 'EOF'
{
  "version": "1.1.0",
  "build_date": "2026-03-10T05:00:00Z",
  "architecture": "x86_64",
  "kernel_version": "6.5.0",
  "rootfs_size_mb": 2048,
  "checksums": {
    "vmlinux_sha256": "...",
    "initrd_sha256": "...",
    "rootfs_sha256": "..."
  }
}
EOF
```

### Test Boot

```bash
# Using vm-manager-cli
./vm-manager-cli setup
./vm-manager-cli start
./vm-manager-cli exec "uname -a"
./vm-manager-cli stop
```

## Publishing

### GitHub Release

```bash
# Create release
gh release create v1.1.0 \
    vmlinux-6.5.0-allternit \
    initrd.img-6.5.0-allternit \
    ubuntu-22.04-allternit-v1.1.0.ext4.zst \
    version-1.1.0-amd64.json \
    --title "VM Images v1.1.0" \
    --notes "Updated Ubuntu 22.04 base with Node.js 20.x"
```

### Manual Distribution

```bash
# Upload to S3
aws s3 cp vm-images/ s3://allternit-vm-images/v1.1.0/ --recursive

# Generate signed URLs
aws s3 presign s3://allternit-vm-images/v1.1.0/ubuntu-22.04-allternit-v1.1.0.ext4.zst
```

---

## See Also

- [Allternit VM README](./Allternit-VM-README.md) - Overview
- [Allternit VM API](./Allternit-VM-API.md) - API Reference
- [CONTRIBUTING.md](../../CONTRIBUTING.md) - Contribution guidelines
