---
title: Installation
---

# Installation

## One-line install (Linux / macOS)

```bash
curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh
```

Downloads the latest release binary for your platform and adds it to your PATH.

## Homebrew (macOS / Linux)

```bash
brew install fagemx/tap/edda
```

## Prebuilt binaries

Download from [GitHub Releases](https://github.com/fagemx/edda/releases).

Available targets:

| Platform | Target |
|----------|--------|
| Linux x86_64 | `edda-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `edda-aarch64-unknown-linux-gnu.tar.gz` |
| macOS x86_64 | `edda-x86_64-apple-darwin.tar.gz` |
| macOS ARM64 | `edda-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `edda-x86_64-pc-windows-msvc.zip` |

Each archive contains the `edda` binary, and a SHA256 checksum sidecar file.

## Build from source

Requires [Rust toolchain](https://rustup.rs/) (1.75+).

```bash
cargo install --git https://github.com/fagemx/edda edda
```

## Verify installation

```bash
edda --version
```

## Next step

Run [`edda init`](./quickstart.md) in your project directory.
