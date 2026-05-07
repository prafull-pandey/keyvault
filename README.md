# KeyVault

A lightweight, cross-platform desktop key/secret/command vault built in Rust with `egui`.
Single statically-linked binary, ~4 MB on Windows, low memory footprint.

## Features

- **Collections** (left sidebar) — add, double-click to rename, drag (`⋮⋮`) to reorder, 🗑 to delete.
- **Tabs** (top bar) — per-collection horizontal tabs, double-click rename, drag to reorder, ✕ to close.
- **Groups** (main panel) — boxed sections with editable name (double-click), drag-reorder, *Add Group* button at the end.
- **Vault items** — three kinds, accessible from the *Add Text ▾* split button at each group's header:
  - **Add Text** — multiline text. Click *Update* to commit, then the field becomes selectable read-only with *📋 Copy* and *✏ Edit* buttons. Label is renameable by double-clicking.
  - **Add Key/Value** — two fields with a copy button next to each, single edit/update toggle, both labels renameable.
  - **Add Replace Text** — a template like `http://{{base_url}}/api/{{path}}`. Define the params below; *📋 Copy* yields the formatted string with placeholders substituted. Use *🔄 Detect params* to auto-add rows for `{{name}}` placeholders found in the template.
- **Persistence** — auto-saved to JSON shortly after every edit, and on app exit. Storage location:
  - Windows: `%APPDATA%\keyvault\keyvault\data\data.json`
  - Linux: `~/.local/share/keyvault/data.json` (XDG)
  - macOS: `~/Library/Application Support/dev.keyvault.keyvault/data.json`

## Build

Requires Rust 1.83+ (some transitive deps need edition2024).

```sh
cargo build --release
./target/release/keyvault        # Linux
./target/release/keyvault.exe    # Windows
```

For development:

```sh
cargo run
```

## Notes on UX

- Text in display mode is also natively selectable, so you can copy parts manually if needed.
- Drag handles are the `⋮⋮` glyph; you can also drag the whole row.
- Status bar in the bottom shows save status, errors, and "copied" toasts.
