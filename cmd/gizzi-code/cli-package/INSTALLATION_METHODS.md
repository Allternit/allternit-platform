# Gizzi Code Installation Methods

Complete reference for installing Gizzi Code. The package is `gizzi-code`, the command is `gizzi`.

## Quick Install

### macOS / Linux

```bash
curl -fsSL https://install.gizziio.com/install | bash
```

### Windows (PowerShell)

```powershell
irm https://install.gizziio.com/install.ps1 | iex
```

---

## Package Managers

### Homebrew (macOS)

```bash
brew tap Allternit/tap
brew install gizzi-code
```

### npm (All Platforms)

```bash
npm install -g @allternit/gizzi-code
```

### Winget (Windows)

```powershell
winget install Allternit.GizziCode
```

### Scoop (Windows)

```powershell
scoop bucket add allternit https://github.com/Allternit/scoop-bucket
scoop install gizzi-code
```

---

## Manual Installation

### Download Binary

1. Download the latest release from GitHub (assets are version-named, e.g.
   `gizzi-code-v2.2.1-<target>.tar.gz`; see
   https://github.com/Allternit/allternit-platform/releases):
   ```bash
   # macOS (Apple Silicon)
   curl -LO https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v2.2.1/gizzi-code-v2.2.1-darwin-arm64.tar.gz

   # Linux (x64)
   curl -LO https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v2.2.1/gizzi-code-v2.2.1-linux-x64.tar.gz

   # Windows (x64)
   curl -LO https://github.com/Allternit/allternit-platform/releases/download/gizzi-code/v2.2.1/gizzi-code-v2.2.1-windows-x64.zip
   ```

2. Make executable (macOS/Linux):
   ```bash
   chmod +x gizzi-code-*
   ```

3. Move to PATH as `gizzi`:
   ```bash
   # macOS/Linux
   mv gizzi-code-* /usr/local/bin/gizzi
   
   # Windows - rename to gizzi.exe and add to PATH
   ```

### Build from Source

```bash
# Clone repository
git clone https://github.com/Allternit/allternit-platform.git
cd gizzi-code/cli-package

# Install dependencies
bun install

# Build
bun run build

# Install globally
npm link
```

---

## Docker

Build the image from the repo (no prebuilt image is published yet):

```bash
git clone https://github.com/Allternit/allternit-platform.git
cd allternit-platform/cmd/gizzi-code
bun run build --target=linux-x64   # produces dist/gizzi-code-linux-x64
docker build -t gizzi-code .

# Run
docker run -it --rm gizzi-code

# With volume mount
docker run -it --rm -v $(pwd):/workspace gizzi-code
```

---

## Verification

After installation, verify with:

```bash
gizzi --version
gizzi --help
```

---

## Post-Installation

### Shell Completions

Add to your shell profile:

**Bash:**
```bash
eval "$(gizzi completion bash)"
```

**Zsh:**
```zsh
eval "$(gizzi completion zsh)"
```

**Fish:**
```fish
gizzi completion fish | source
```

### System Service

**macOS:**
```bash
gizzi service install
gizzi service start
```

**Linux:**
```bash
gizzi service install --user
gizzi service start
```

---

## Uninstallation

### curl/bash Install

```bash
rm ~/.local/bin/gizzi
rm -rf ~/.config/gizzi
```

### Homebrew

```bash
brew uninstall gizzi-code
brew untap allternit/tap
```

### npm

```bash
npm uninstall -g @allternit/gizzi-code
```

### Winget

```powershell
winget uninstall Allternit.GizziCode
```

---

## Troubleshooting

### Command not found

```bash
# Check if in PATH
which gizzi

# Add to PATH
export PATH="$HOME/.local/bin:$PATH"
```

### Permission denied

```bash
chmod +x $(which gizzi)
```

### Outdated version

```bash
# Update via your package manager
brew upgrade gizzi-code       # Homebrew
npm update -g @allternit/gizzi-code # npm
winget upgrade Allternit.GizziCode  # Winget
```

---

## Support

- **Documentation**: https://docs.gizziio.com
- **Issues**: https://github.com/Allternit/allternit-platform/issues
- **Discord**: https://discord.gg/allternit
