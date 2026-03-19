# RetroTLS

Ultra-lightweight HTTP to HTTPS bridge proxy for legacy clients.

## Overview

RetroTLS is a minimal, high-performance, single-binary bridge proxy that allows legacy HTTP clients to connect to modern HTTPS APIs. It listens for plain HTTP requests and forwards them to HTTPS upstream servers.

> **Korean Documentation**: [README.md](README.md)

### Key Features

- **Minimal**: Single purpose, single binary (~2.3MB)
- **Fast**: Async I/O with Tokio, streaming bodies, connection pooling
- **Secure**: TLS 1.2+ only, certificate verification
- **Simple**: YAML configuration, no web UI, no complex features

## Installation

### One-Line Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

Or with wget:

```bash
wget -qO- https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

If `~/.local/bin` is not in your PATH, add:
```bash
export PATH="$HOME/.local/bin:$PATH"
```

### Update

Same as install. Automatically updates to the latest version:
```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

### Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh -s -- --uninstall
```

### Manual Installation

Download the binary for your OS from the releases page:  
https://github.com/parkjangwon/retrotls/releases

### From Source

```bash
git clone https://github.com/retrotls/retrotls
cd retrotls
cargo build --release
```

## Usage

### Quick Start

1. Create configuration directory:
```bash
mkdir -p ~/.config/retrotls
```

2. Create a configuration file at `~/.config/retrotls/config.yaml`:
```yaml
access_log: true
listeners:
  - listen: "127.0.0.1:8080"
    upstream: "https://api.example.com"
```

3. Run RetroTLS:
```bash
retrotls
```

### CLI Options

```
retrotls [OPTIONS]

Options:
  -c, --config <FILE>    Configuration file path
      --check            Validate configuration and exit
      --version          Print version
      --log-level <LEVEL> Log level (debug, info, warn, error)
  -h, --help             Print help
```

### Example Request Flow

Client request:
```bash
curl http://127.0.0.1:8080/users?id=1
```

Forwarded to upstream as:
```
https://api.example.com/users?id=1
```

## Configuration

Configuration file path: `~/.config/retrotls/config.yaml`

### Full Example

```yaml
access_log: true

listeners:
  - listen: "127.0.0.1:8080"
    upstream: "https://api1.com"
  
  - listen: "127.0.0.1:8081"
    upstream: "https://api2.com/base"
```

### Configuration Options

#### Listeners

- `listen`: Socket address to listen on (e.g., "127.0.0.1:8080")
- `upstream`: HTTPS URL to forward requests to (must start with "https://")

Path handling examples:
- Client: `/v1/test` → Upstream: `https://api.com/base/v1/test`
- Client: `/` → Upstream: `https://api.com/base/`

#### Logging

- `access_log`: Enable access logging (default: true)

Access log format:
```
<timestamp> <client_addr> -> <bind_addr> <method> <path> <status> <latency_ms>ms
```

## Example

Test RetroTLS in the `example/` directory:

```bash
cd example

# Run RetroTLS (Terminal 1)
../target/release/retrotls --config config.yaml

# Run tests (Terminal 2)
./test.sh

# Or manual test
curl http://127.0.0.1:8080/get
curl -X POST http://127.0.0.1:8080/post -H "Content-Type: application/json" -d '{"test": "hello"}'
```

## Systemd Service

A user systemd service file is provided (`retrotls.service`):

```bash
cp retrotls.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable retrotls
systemctl --user start retrotls
systemctl --user status retrotls
journalctl --user -u retrotls -f
```

## Building

### Development Build
```bash
cargo build
```

### Release Build (Optimized)
```bash
cargo build --release
```

### Running Tests
```bash
cargo test
```

## Supported Features

- HTTP/1.1 request forwarding
- HTTPS upstream with TLS 1.2/1.3
- Request/response streaming
- Connection pooling and keep-alive
- Hop-by-hop header filtering
- X-Forwarded-* headers
- Graceful shutdown (SIGINT/SIGTERM)

## Architecture

RetroTLS follows a minimal architecture:

1. **HTTP Listener**: Binds to configured addresses
2. **Request Handler**: Forwards HTTP requests to HTTPS upstream
3. **TLS Client**: Establishes secure connections to upstream
4. **Response Streamer**: Returns upstream responses to clients

## Security Considerations

- Always use TLS 1.2 or higher
- Keep certificate verification enabled in production
- Bind to localhost (127.0.0.1) unless you have specific requirements
- Run without root privileges
- No sensitive data is logged by default

## License

MIT License - See LICENSE file for details.

## Troubleshooting

### "Failed to load config"
Check that the configuration file exists at `~/.config/retrotls/config.yaml` or specify with `--config`.

### "Bind failed"
Ensure the port is not already in use and you have permission to bind to it.

### "Upstream connection failed"
Verify the upstream URL is correct and accessible from the RetroTLS host.

### "Gateway Timeout"
Increase timeout settings in the configuration if your upstream is slow.

---

**RetroTLS** - A small, solid bridge connecting legacy clients to modern APIs
