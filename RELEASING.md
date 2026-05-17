# Release Process

This document describes the release process for oxo-flow.

## Version Numbering

oxo-flow follows [Semantic Versioning 2.0.0](https://semver.org/):

- **MAJOR** (`X.0.0`) — Incompatible API changes or breaking changes to the
  `.oxoflow` file format.
- **MINOR** (`0.X.0`) — New features and functionality added in a
  backwards-compatible manner.
- **PATCH** (`0.0.X`) — Backwards-compatible bug fixes and security patches.

### Pre-release Versions

Pre-release versions use suffixes: `1.0.0-alpha.1`, `1.0.0-beta.1`,
`1.0.0-rc.1`.

### Version Locations

The version is maintained in:

- `Cargo.toml` (workspace root)
- `crates/*/Cargo.toml` (each workspace member)
- `CITATION.cff`

All version references must be updated together.

## Release Checklist

Before creating a release, ensure the following:

- [ ] All CI checks pass on the `main` branch
- [ ] `cargo fmt -- --check` reports no formatting issues
- [ ] `cargo clippy -- -D warnings` reports zero warnings
- [ ] `cargo build` succeeds for all workspace members
- [ ] `cargo test` passes all unit and integration tests
- [ ] Version numbers are updated in all `Cargo.toml` files and `CITATION.cff`
- [ ] `CHANGELOG.md` is updated with all changes since the last release
- [ ] Documentation is updated to reflect any new features or changes
- [ ] Breaking changes are clearly documented with migration instructions
- [ ] Security advisories for resolved vulnerabilities are prepared (if any)

## Changelog Generation

oxo-flow uses [git-cliff](https://github.com/orhun/git-cliff) for automated
changelog generation. Configuration is in `cliff.toml`.

### Commit Message Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`,
`ci`, `chore`.

### Generating the Changelog

```bash
# Preview changelog for the next release
git cliff --unreleased

# Generate full changelog
git cliff -o CHANGELOG.md

# Generate changelog for a specific version
git cliff --tag v1.0.0 -o CHANGELOG.md
```

## Publishing Steps

### 1. Prepare the Release

```bash
# Ensure you are on the main branch and up to date
git checkout main
git pull origin main

# Run the full CI suite locally
make ci

# Update version numbers
# Edit Cargo.toml (workspace and crate versions), CITATION.cff

# Generate changelog
git cliff --tag vX.Y.Z -o CHANGELOG.md

# Review the changelog and make any manual edits
```

### 2. Create the Release Commit

```bash
git add -A
git commit -m "chore: release vX.Y.Z"
```

### 3. Tag the Release

```bash
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin main --tags
```

### 4. Publish to crates.io

Publish workspace members in dependency order:

```bash
cargo publish -p oxo-flow-core
cargo publish -p oxo-flow-cli
cargo publish -p oxo-flow-web
```

### 5. Create the GitHub Release

- Go to the [Releases page](https://github.com/Traitome/oxo-flow/releases)
- Click **Draft a new release**
- Select the tag `vX.Y.Z`
- Copy the relevant section from `CHANGELOG.md` into the release notes
- Attach pre-built binaries if available
- Publish the release

### 6. Post-Release

- Announce the release on project communication channels
- Update documentation site if applicable
- Verify the published crates on [crates.io](https://crates.io/)
- Bump version numbers in `Cargo.toml` to the next development version
  (e.g., `X.Y.(Z+1)-dev`)

## Hotfix Releases

For urgent bug fixes or security patches:

1. Create a branch from the release tag: `git checkout -b hotfix/vX.Y.Z vX.Y.Z`
2. Apply the fix and add tests
3. Follow the standard release process from step 2 onward

---

This project is licensed under the [Apache License 2.0](LICENSE).
