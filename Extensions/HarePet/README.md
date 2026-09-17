# Hare Pet extension

This development extension attaches the Codex Hare pet app to Yeet's extension protocol. The app is expected at `~/Code/Swift/CodexPetTest/dist/CodexPetTest.app`.

Install it globally with:

```sh
cargo run -- extension install Extensions/HarePet --force
```

When a Yeet workspace background daemon starts, the extension launches automatically on macOS. It remains a single companion even when that daemon hosts multiple session runtimes. In extension mode the Swift app switches to a transparent floating pet window and maps live Yeet state to Hare animations.
