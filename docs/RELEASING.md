# Releasing Yeet

Release tags use `v<version>`. The version must match `Cargo.toml`, `RuntimeSource/package.json`, and `web/package.json`.

## Prepare

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
npm --prefix web ci
npm --prefix web run build
./Scripts/package-release.sh
```

The packaging script checks the staged version, runs `yeet doctor`, verifies the bundled `install.sh` through a temporary-prefix install, and then creates the archive.

## Publish

For version `0.1.0`:

```sh
git tag -a v0.1.0 -m "Yeet v0.1.0"
git push origin v0.1.0
```

The release workflow runs a focused functional gate (`cargo test --lib --bins -- --test-threads=1`, RuntimeSource checks, and a WebUI build), then builds macOS, Linux, and Windows packages for ARM64 and x64. Rust release tests are serialized because the Remote lifecycle tests intentionally override process-global Yeet configuration while exercising detached child processes; parallel execution can interfere with unrelated tests that instantiate `ConfigStore`. The source-layout architecture guard remains enforced by normal push/PR CI instead of blocking tag packaging on file-length policy. Unix release archives include the binary installer and bundled runtime; Linux jobs also produce `.deb` packages. Release assets include per-file SHA-256 sidecars, an aggregate `SHA256SUMS`, and GitHub provenance attestations.

The publish job renders `Formula/yeet.rb` from the four macOS/Linux archive checksums and updates `kyooni18/homebrew-tap` when the Yeet repository secret `HOMEBREW_TAP_TOKEN` is configured with write access to that tap. If the secret is absent, the GitHub Release still succeeds and the workflow reports that tap publication was skipped.

Windows packages are Authenticode-signed when signing secrets are configured. macOS Developer ID notarization is not currently configured.

After publishing, verify both installation routes:

```sh
curl -fsSL https://raw.githubusercontent.com/kyooni18/Yeet/main/install.sh | sh
brew install kyooni18/tap/yeet
yeet --version
yeet doctor
yeet update check
```
