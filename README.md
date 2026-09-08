# Yeet

Yeet is a Rust-native terminal AI agent with separate general-purpose and coding
modes. It supports multiple model providers, tools, MCP servers, skills, file
editing, sessions, and an optional browser-based remote UI.

## Requirements

- Rust stable (including `cargo`)
- Node.js 20 or newer
- npm

Rust builds the application; Node runs Yeet's TypeScript provider and tool
runtime.

## Install

```sh
git clone https://github.com/Yeet-AI/Yeet.git
cd Yeet
npm --prefix RuntimeSource install
./Scripts/install-local.sh
```

The installer builds Yeet and installs it to a local executable directory. Add
that directory to your `PATH` if needed. To build without installing:

```sh
npm --prefix RuntimeSource install
./Scripts/rebuild-runtime.sh
cargo build --release
```

Run `yeet` to start the TUI, or `yeet doctor` to check the setup. Configure a
model provider and credentials from the app or your environment before sending
requests. Use `yeet remote` only when you want to expose the UI in a browser.

## Development

```sh
cargo test
npm --prefix RuntimeSource test
```
