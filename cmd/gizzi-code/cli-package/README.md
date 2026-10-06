# Gizzi Code

AI-powered terminal interface and runtime for the Allternit ecosystem.

## Installation

### Quick Install (macOS/Linux)

```bash
curl -fsSL https://install.gizziio.com/install | bash
```

### Homebrew (macOS)

```bash
brew tap Allternit/tap
brew install gizzi-code
```

(Formula: https://github.com/Allternit/allternit-platform/blob/main/cmd/gizzi-code/packaging/homebrew/gizzi-code.rb)

### npm

```bash
npm install -g @allternit/gizzi-code
```

### Windows

```powershell
# Using winget
winget install Allternit.GizziCode

# Using scoop
scoop bucket add allternit https://github.com/Allternit/scoop-bucket
scoop install gizzi-code
```

## Usage

### Start the TUI

```bash
gizzi
```

### Commands

```bash
gizzi --version        # Show version
gizzi --help           # Show help
```

## System Service Setup

### macOS

```bash
# Install LaunchAgent
cp install/com.allternit.gizzi.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.allternit.gizzi.plist
```

### Linux (systemd)

```bash
# Install user service
mkdir -p ~/.config/systemd/user/
cp install/gizzi.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable gizzi.service
systemctl --user start gizzi.service
```

## Configuration

Configuration is stored at:
- **macOS**: `~/.config/gizzi/config.json`
- **Linux**: `~/.config/gizzi/config.json`
- **Windows**: `%APPDATA%/gizzi/config.json`

## Documentation

- [Full Documentation](https://docs.gizziio.com)
- [API Reference](https://docs.gizziio.com/api)
- [Troubleshooting](https://docs.gizziio.com/troubleshooting)

## License

Apache License 2.0. See [LICENSE](../LICENSE) and [NOTICE](../NOTICE).
