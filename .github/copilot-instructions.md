# Tile — Agent & Review Instructions

## Reference Map (Read Before Changing That Area)

- `PRODUCT.md` — product scope, capabilities, principles, and voice (any user-facing behavior or copy change).
- `DESIGN.md` — visual tokens, One Blue / Three Tones / Semantic Pair rules (any UI/CSS change).
- `GLOSSARY.md` — canonical tiling terms (**Base modifier**, **Display modifier**, **Side placement**, **Corner placement**, **Corner composition**, **Center column**, **Size cycle**). Use these terms in UI, docs, and comments.
- `docs/adr/0001-minimal-keymap-and-size-cycling.md` — minimal 9-action keymap + 2 group modifiers + size cycle + single-hold composition decision.
- `CONTRIBUTING.md` — local prerequisites, Manual QA checklist, and `BuildKind` dev-build rules.
- `docs/RELEASING.md` — version bump flow, signing/notarization, updater `pubkey`, and `latest.json` rules.

## Architecture & Cross-Cutting Invariants

- **3-crate boundary:**
  - `crates/tile-core` — pure, platform-independent geometry, actions, animation math, history, and config (zero OS calls or I/O beyond serde JSON).
  - `crates/tile-platform` — OS backends (Windows / macOS + `unsupported` fallback) behind `WindowBackend` and `HotkeyBackend`.
  - `apps/tile` — thin Tauri v2 shell (`apps/tile/src`) + vanilla TypeScript/Vite UI (`apps/tile/ui`).
- **Coordinate units:** Windows uses physical pixels (per-monitor DPI aware v2); macOS uses logical points. Never scale coordinates in `tile-core`, and avoid calling backend-native distances universally "px" in cross-platform UI or docs.
- **Minimal keymap (ADR 0001):** Keep exactly 9 bindable `WindowAction`s across 2 `ShortcutGroup`s (`Placement`, `Displays`), width cycling via `CycleSize`, and single-hold corners via `CornerComposer`. Do not expand into separate fractional-grid or direct-corner bindings.
- **Dev/installed quarantine (`TILE_BUILD_KIND`):** `apps/tile/src/build_kind.rs` (`BuildKind`) isolates local builds (`Tile-Development` config dir, no OS login-item writes, no single-instance lock, `(Development)` labels) from release builds (`TILE_BUILD_KIND=installed`). Encode new differences as methods on `BuildKind`, not call-site branches.
- **Tauri & version parity:** Each `@tauri-apps/api` and `@tauri-apps/plugin-*` package in `apps/tile/ui/package-lock.json` must share `major.minor` with its corresponding `tauri` / `tauri-plugin-*` crate in `Cargo.lock` (enforced by `apps/tile/ui/scripts/check-tauri-versions.mjs`; `@tauri-apps/cli` is not part of this pair check). Keep app versions equal across `Cargo.toml`, `apps/tile/tauri.conf.json`, and `apps/tile/ui/package.json`.
- **Toolchain & MSRV:** Rust `1.77.2` (`Cargo.toml`, `clippy.toml`), stable toolchain only, formatted with `rustfmt`, zero clippy warnings (`-D warnings`).

## Lean Diff / YAGNI

- Deliver the smallest complete change that solves the task. Reuse existing primitives (`SettingsTransaction`, `saveSetting`, `BuildKind`, `tile_core` types) instead of adding speculative abstractions, config knobs, or dependencies.
- Preserve all security, accessibility (`prefers-reduced-motion`, keyboard focus), data-loss (`config_store` backup/rollback), and OS/hardware calibration guards (DWM border delta, AX timeouts/Enhanced UI guard, Windows hook recovery).
- Mark deliberate deferrals with `// ponytail: <ceiling>, <upgrade trigger>` (never `// shortcut:`, which collides with Tile's domain terms).

## Verification Commands

### Linux / Copilot Cloud Agent (matches `.github/workflows/ci.yml` `core` + `cross-check`)

```sh
cargo fmt --all -- --check
cargo clippy -p tile-core -p tile-platform --all-targets -- -D warnings
cargo test -p tile-core -p tile-platform
cargo check -p tile-platform --target aarch64-apple-darwin --tests
cargo check -p tile-platform --target x86_64-apple-darwin --tests
cargo check -p tile-platform --target x86_64-pc-windows-msvc --tests
cd apps/tile/ui && node scripts/check-tauri-versions.mjs && npm run build
```

### Windows / macOS (matches `.github/workflows/ci.yml` `desktop`)

```sh
cd apps/tile/ui && npm ci && node scripts/check-tauri-versions.mjs && npm run build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace
```
