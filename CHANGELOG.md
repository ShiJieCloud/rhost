# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/).

[中文](./CHANGELOG.zh-CN.md) · **English**

## [2.0.0] - 2026-10-15

This release is centered on **a full AI assistant**: Rhost evolves from a pure SSH terminal manager into a remote host workbench with a built-in AI assistant, supporting conversational terminal operations, Plan-and-Act task planning and multimodal input.

### Added

#### AI assistant

- Complete AI assistant: streaming chat, Markdown and code rendering, code blocks with line numbers plus insert-to-terminal / execute buttons
- Message actions: copy and regenerate for assistant messages; edit-and-resend, copy and delete for user messages
- Reasoning visualization: the thinking process is collapsed independently with a "Thinking…" pulse animation and can be expanded / collapsed manually
- Multimodal input: add image and text file attachments by picker, paste or drag-and-drop
- Chat outline navigation: jump quickly within long conversations
- Context injection: AI skills, rules and cross-session memory injected into the system prompt, with per-file toggles, injection budget control and settings-panel management
- Terminal toolset: read-only tools (read file, list directory, system info) run automatically without approval; file edits and command proposals require user approval, with automatic backup before edits and rejection of critical system paths; built-in high-risk command detection
- Command output feedback toggle (off by default to keep the zero-feedback privacy boundary); when enabled, the AI can perceive execution results and act autonomously over multiple steps
- Plan-and-Act task planning: the model drafts a plan for complex tasks, executes it step by step after user confirmation with live per-step status; failed steps can be retried, skipped, aborted or re-planned; a plan converges early once the goal is met or the remaining steps become infeasible
- File upload / download tools: the AI can perform SFTP transfers directly, with a 64MB size pre-check and a clear error guiding users to the SFTP pane when exceeded

#### Workbench and UI

- Right panel: a combined layout for recent sessions, server monitoring and the AI assistant, with a collapsible icon rail kept in sync between the home page and the workbench
- Recent sessions and quick connect: successful connections are recorded automatically and can be reconnected with one click
- Cross-platform title bar adaptation: native traffic lights and custom-drawn buttons are switchable

### Changed

- AI module settings (including API keys) are stripped from configuration import / export
- Release builds disable the WebView native context menu and default shortcuts to avoid overlapping the custom menu; debug builds keep the debugging shortcuts
- Terminal command execution no longer discards output on timeout — partial stdout / stderr already received is preserved
- The API key input supports show / hide toggling
- Large-scale documentation site and README build-out: guides, design documents and architecture decision records (ADRs)

### Fixed

- Residual `/tmp/.ri-*` scripts from AI terminal execution are cleaned up when a session disconnects
- The tab bar enables drag scrolling and the grab cursor only when tabs actually overflow

## [1.0.0] - 2026-10-08

First public release: a cross-platform SSH remote host manager built with Tauri + Vue 3.

### Added

- Core SSH terminal: multi-tab session management with terminal font and cursor settings applied live
- SSH port forwarding: local, remote and dynamic modes
- SSH automatic reconnect on disconnect plus a startup animation
- SSH host key verification (TOFU) with a fingerprint confirmation dialog
- Tab bar context menu and tunnel configuration
- Host groups, key management and home view switching
- Configuration import / export (mandatory PBKDF2-SHA256 + AES-256-GCM encryption)
- SFTP transfer queue: collapsed progress summary and state persisted across restarts
- Custom MOTD ASCII logo
- A complete application logging system
- Settings dialog and the full set of application icons

### Fixed

- Fixed a remote prompt missing the username

[2.0.0]: https://github.com/ShiJieCloud/rhost/compare/v1.0.0...v2.0.0
[1.0.0]: https://github.com/ShiJieCloud/rhost/releases/tag/v1.0.0
