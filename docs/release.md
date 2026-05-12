# Release and Install

## Release Builds

Release builds are produced by `.github/workflows/release.yml` for:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Each build runs:

```bash
cargo build --release -p scheduler-cli --target <target>
./target/<target>/release/scheduler --version
```

Tagged releases matching `v*` publish `.tar.gz` archives containing the
`scheduler` binary.

## Install Script

Install from the latest GitHub release:

```bash
curl -fsSL https://raw.githubusercontent.com/example.invalid/scheduler-cli/main/scripts/install.sh | sh
```

Override the repository, version, or install directory:

```bash
SCHEDULER_REPO=owner/scheduler-cli \
SCHEDULER_VERSION=v0.1.0 \
SCHEDULER_INSTALL_DIR="$HOME/.local/bin" \
sh scripts/install.sh
```

After installation:

```bash
scheduler setup
scheduler provider list
```

## Local Install

From a checkout:

```bash
cargo install --path crates/scheduler-cli
scheduler setup
```

## macOS Signing

macOS signing and notarization are optional in the release workflow. They run on
macOS release jobs when these repository secrets are configured:

- `MACOS_CERTIFICATE_P12_BASE64`
- `MACOS_CERTIFICATE_PASSWORD`
- `MACOS_KEYCHAIN_PASSWORD`
- `MACOS_CODESIGN_IDENTITY`
- `MACOS_NOTARY_APPLE_ID`
- `MACOS_NOTARY_PASSWORD`
- `MACOS_NOTARY_TEAM_ID`

Without those secrets, macOS archives are built unsigned. With those secrets,
the workflow imports the Developer ID certificate, signs the `scheduler` binary,
verifies the signature, and submits a zip containing the binary to Apple
notarytool before packaging the release archive.

## Release Checklist

- Update `CHANGELOG.md`.
- Confirm migrations and compatibility notes are included.
- Run the CI gate locally.
- Tag as `vMAJOR.MINOR.PATCH`.
- For macOS distribution outside developer/test channels, confirm signing
  secrets are configured and the notarization step completed.
- Confirm uploaded archives run `scheduler --version`.
