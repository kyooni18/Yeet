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
pnpm --dir web install --frozen-lockfile
pnpm --dir web build
./Scripts/package-release.sh
```

The packaging script checks the staged version and runs `yeet doctor` before creating the archive.

## Publish

For version `0.1.0`:

```sh
git tag -a v0.1.0 -m "Yeet v0.1.0"
git push origin v0.1.0
```

The release workflow runs one focused check job (version consistency, Rust tests, RuntimeSource checks, and a WebUI build), then builds macOS, Linux, and Windows packages for ARM64 and x64. Broader CI such as Playwright, Docker, duplicate platform test matrices, and CLI smoke tests stays on normal pushes and pull requests instead of blocking tag packaging. Release assets include checksums and GitHub provenance attestations.

Windows packages are Authenticode-signed when signing secrets are configured. macOS Developer ID notarization is not currently configured.

After publishing, verify a clean installation with `yeet --version`, `yeet doctor`, and `yeet update check`.
