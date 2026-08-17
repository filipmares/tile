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
