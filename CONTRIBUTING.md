# Contributing to Tile

Thanks for your interest in Tile! Contributions of all kinds are welcome — bug
reports, documentation fixes, and code.

## Ground rules

- Be respectful and constructive.
- Keep pull requests focused; small, reviewable changes merge faster.
- By contributing, you agree that your contributions are licensed under the
  [MIT License](LICENSE).

## Project layout

Tile is a Cargo workspace with a deliberately layered design:

| Crate / path         | Responsibility                                                    |
| -------------------- | ----------------------------------------------------------------- |
| `crates/tile-core`   | Pure, platform-independent logic (geometry, actions, config).     |
| `crates/tile-platform` | OS backends (Windows / macOS) behind traits, plus a fallback.   |
| `apps/tile`          | Thin [Tauri v2](https://v2.tauri.app/) desktop shell + Vite UI.   |

Keep `tile-core` free of any platform or I/O code — that is what makes the
behaviour of the app testable on any host, including Linux CI.

## Prerequisites

- **Rust** (stable) — install via [rustup](https://rustup.rs/).
- **Node.js 18+** and npm — for the frontend under `apps/tile/ui`.
- **Windows:** Visual Studio C++ Build Tools + Windows SDK (for `link.exe`) and
  the WebView2 runtime.
- **macOS:** Xcode Command Line Tools (`xcode-select --install`).

## Development workflow

Before opening a pull request, run the same checks CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For the desktop app:

```sh
cd apps/tile/ui && npm ci && npm run build
cargo build --workspace
```

> **Note:** CI is the only place the macOS build is compiled if you are working
> on Windows (and vice versa). Push early and watch the
> [CI workflow](.github/workflows/ci.yml) — `cargo clippy --all-targets` is what
> catches breakage inside the other platform's `#[cfg]` and `#[cfg(test)]` code.

## Development builds

Every build that is not produced by the release workflow is a **development
build** — including a local `tauri build`, which is a release *profile* but not
an installed app. `debug_assertions` cannot tell those apart, so provenance is
decided by the `TILE_BUILD_KIND` environment variable, which only
[`release.yml`](.github/workflows/release.yml) sets (`installed`). Anything
else — unset, empty, misspelled — is classified as a development build, so a
broken CI variable degrades a release rather than letting a checkout act like
one.

Classification and everything that hangs off it live in
[`apps/tile/src/build_kind.rs`](apps/tile/src/build_kind.rs); the kind is
resolved once at startup and stored on `AppState`. A development build:

- reads and writes its config in a separate `Tile-Development` directory, so it
  cannot clobber the config of an installed Tile;
- never enables or disables the OS login item — the `launchOnLogin` preference
  is still persisted, just not applied (see `apps/tile/src/autostart.rs`);
- labels itself in the tray menu, the tray tooltip, the settings window title
  and a panel in the settings UI.

If you add behaviour that should differ between the two, add a method to
`BuildKind` rather than branching on the enum at the call site, and cover it
with a unit test — the classification, directory-name and label logic are all
pure and tested in `build_kind.rs`.

To exercise the installed path locally from the repository root, build with the
variable set:

```sh
cd apps/tile && TILE_BUILD_KIND=installed ./ui/node_modules/.bin/tauri build
```

Note that it will then use, and write to, the real config directory and the real
login item.

## Formatting and lints

- Rust code is formatted with `rustfmt` (see `rustfmt.toml`) and must be
  warning-free under `clippy` with `-D warnings`.
- Please do not introduce nightly-only tooling; CI runs the stable toolchain.

## Commit messages

Write clear, imperative commit messages (e.g. "Add center-window action").

## Releasing

Maintainers cut releases by pushing a `v*` tag. The signing, notarization and
publishing steps are documented in [`docs/RELEASING.md`](docs/RELEASING.md).

## Reporting bugs

Open an issue using the templates in
[`.github/ISSUE_TEMPLATE`](.github/ISSUE_TEMPLATE). Include your OS and version,
what you expected, and what happened.
