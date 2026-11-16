# Releasing Yeet

Release tags use `v<version>`. The version must match `Cargo.toml`, `RuntimeSource/package.json`, and `web/package.json`.

## Prepare

The experimental compatibility WebUI requires a built Mün workspace. Set `MUN_PATH` to its absolute path (for example, `MUN_PATH=/Users/kyooni18/Code/misutgaru/packages/modules/Mun`). CI builds the compatible revision pinned in `.github/actions/setup-mun/action.yml` and exports `MUN_PATH` automatically. Export this variable before a local package build or an opt-in WebUI source install. Both packaging scripts build the compatibility WebUI and embed it in every release binary; the release workflow sets up the pinned Mün workspace on each packaging runner. End users of prebuilt packages need neither Mün nor a frontend build.

1. Update the three package versions and `CHANGELOG.md`.
2. Commit the intended release with a clean working tree.
3. Run:

```sh
node Scripts/verify-release-version.mjs
cargo fmt --all -- --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
npm --prefix RuntimeSource ci
npm --prefix RuntimeSource run check
./Scripts/package-release.sh
```

The packaging script checks the staged version, runs `yeet doctor`, tests the bundled installer lifecycle in mocked process fixtures (without stopping host services), and then creates the archive.

## Publish

For version `0.1.0`:

```sh
git tag -a v0.1.0 -m "Yeet v0.1.0"
git push origin v0.1.0
```

The release workflow runs a focused functional gate (`cargo test --lib --bins -- --test-threads=1`, and RuntimeSource checks), then queues independent macOS, Linux, and Windows platform builds (including Linux i686). Each successful job immediately uploads its packages and SHA-256 sidecars to a prerelease, so other platforms do not block it. The prerelease becomes visible once the first platform finishes; the previous stable release remains `latest` until all platforms and the finalization job succeed. Re-run failed jobs to complete a partial release. GitHub provenance attestations accompany the platform assets.

To install an already completed Unix platform from a partial prerelease without waiting for the other platforms, explicitly select its tag (the default installer uses the latest stable release):

```sh
curl -fsSL https://raw.githubusercontent.com/kyooni18/Yeet/main/install.sh | YEET_VERSION=v0.1.0 sh
```

Rust release tests are serialized because the Remote lifecycle tests intentionally override process-global Yeet configuration while exercising detached child processes; parallel execution can interfere with unrelated tests that instantiate `ConfigStore`. The source-layout architecture guard remains enforced by normal push/PR CI instead of blocking tag packaging on file-length policy. Unix archives include the binary installer and bundled runtime; Linux jobs also produce `.deb` packages.

The finalization job generates and attests aggregate `SHA256SUMS`, renders `Formula/yeet.rb` from the four macOS/Linux archive checksums, and updates `kyooni18/homebrew-tap` over SSH using the repository secret `HOMEBREW_TAP_DEPLOY_KEY` before promoting the release to stable. That secret contains a dedicated Ed25519 deploy key whose public half has write access only to the tap repository; no personal access token is required. Tap publication is a required release step, so a missing or invalid deploy key fails the release instead of silently leaving Homebrew stale.

Windows packages are Authenticode-signed when signing secrets are configured. macOS Developer ID notarization is not currently configured.

After publishing, verify both installation routes:

```sh
curl -fsSL https://raw.githubusercontent.com/kyooni18/Yeet/main/install.sh | sh
brew install kyooni18/tap/yeet
yeet --version
yeet doctor
yeet update check
```
