---
name: Alfred terminal project
description: Core facts about the Alfred project — a Rust terminal multiplexer for Windows/Linux inspired by cmux
type: project
---

Project is called **Alfred**. Located at `C:\Users\rahza\Documents\work\cmux-windows\alfred\`.

Why: The working directory is `cmux-windows` (can't rename it while Claude Code is running there), so the actual project lives in a subfolder called `alfred`.

**Goal:** GPU-accelerated terminal multiplexer for Windows and Linux, inspired by cmux (macOS). Key features: tabs + split panes, sidebar for project switching, AI agent notifications (OSC 777/9/99), CLI tool (`alfred`).

**Tech stack:**
- Language: Rust
- Windowing: winit
- GPU: wgpu 28 — DX12 on Windows (AMD stability), Vulkan on Linux
- Text rendering: glyphon + cosmic-text + swash
- Terminal model: alacritty_terminal 0.24
- VT parser: vte 0.15
- PTY: portable-pty (patched fork for ConPTY flags)
- Async: tokio
- Git: git2
- CLI parsing: clap

**Hardware:** AMD Radeon(TM) Graphics (integrated iGPU), 512MB VRAM. Must use DX12 explicitly — AMD iGPU is misreported as DiscreteGpu in wgpu DX12 backend, enumerate by name.

**Ghostty:** NOT reusing libghostty — Windows build broken, requires Zig toolchain, API unstable.

**Build phases:** (1) core terminal, (2) multiplexing, (3) sidebar, (4) AI notifications, (5) IPC/CLI.

**Architecture doc:** `alfred/ARCHITECTURE.md`

**Why:** Inspired by cmux for macOS. User wants it for AI agent workflows (Claude Code etc.) on Windows, eventually Linux too.

**How to apply:** All work goes in `cmux-windows/alfred/`. Binary names are `alfred-app` and `alfred-cli`. Config path: `%APPDATA%\alfred\config.toml` (Windows), `~/.config/alfred/config.toml` (Linux). IPC: `\\.\pipe\alfred` (Windows), `/tmp/alfred.sock` (Linux).
