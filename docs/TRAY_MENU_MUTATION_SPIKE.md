# Tray menu mutation spike

## Result

Runtime tray text mutation is practical. On macOS, with Tauri 2.11.5 and muda
0.19.3, a retained `MenuItem<R>` changed from `Check for Updates…` to
`Update to 0.2.1…` after the tray had been built.

The executable probe is preserved in commit
`3cb3c8352c56a2011d8e4b25a1cb96806cc6be50`; it was removed from the branch tip
because the real update UI supersedes it.

The probe called `MenuItem::set_text`, `text`, `set_enabled`, and `is_enabled`
directly from a named background thread. All calls succeeded and the changed
label appeared in the live tray menu. Tauri's `MenuItem` wrapper is `Send` and
`Sync`; each operation synchronously calls `AppHandle::run_on_main_thread`
before touching muda. Tile must not call muda directly, but it does not need an
additional `run_on_main_thread` layer around Tauri's menu API.

The same API is also safe from Tauri's main thread, including a tray
`on_menu_event` callback. The menu wrapper waits synchronously on `rx.recv()`
after asking `run_on_main_thread` to perform the mutation, which would deadlock
if an already-main-thread call were merely queued. The Wry runtime prevents
that: `send_user_message` compares `current_thread().id()` with
`context.main_thread_id` and calls `handle_user_message` inline when they match.
The probe exercises the worker-thread path; this inline-dispatch guard in Tauri
2.11.5 establishes the main-thread path.

Check out the proof commit, then run the opt-in probe with:

```sh
TILE_TRAY_MUTATION_SPIKE=1 RUST_LOG=info cargo run -p tile
```

After one second the temporary update item changes label. The log reports the
text read-back and the disable/re-enable round trip. Without the environment
variable, the probe item is not added to the tray.

## Ownership and recommendation

Keep retained handles in a dedicated Tauri-facing `TrayUi<R>` created during
`setup` and registered as managed state. Do not put them in `AppState`: doing so
would make otherwise runtime-agnostic domain state generic over `Runtime`.

Use one always-present update item in the finished UI and mutate its text when
background discovery finds a release. `set_enabled` is available and works
from a worker thread, but is not required merely to change status; it is useful
only when duplicate clicks must be prevented during an in-flight operation.

There is no `MenuItem::set_visible` API in Tauri or muda. A parent menu can
remove and reinsert an item, and Tauri marshals those structural mutations too,
but that adds ordering and separator bookkeeping with no benefit for this UX.
Prefer changing text and enabled state instead of hiding the item.

If a future Tauri or platform regression makes tray mutation impractical, use a
static `Check for Updates…` item and put update status and actions in the
existing About window. That is a stronger fallback than the settings window:
the About window already shows Tile's version, and placing update controls there
matches conventional macOS application UX. Background checks must still avoid
dialogs; at most one dialog per discovered version remains appropriate.

The About window cannot invoke Tauri commands today. Its label is `about`, while
`apps/tile/capabilities/default.json` grants permissions only to `settings`.
Any implementation that puts an update action in About must first extend the
appropriate capability to include the About window. This spike does not make
that capability change because the dynamic tray path works.
