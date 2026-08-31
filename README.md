# tinynote

Raycast Notes in your terminal. A tiny markdown notes TUI over plain files.

Your notes are a flat folder of `.md` files in `~/notes` — nothing else. No folders, no tags, no sync, no accounts, no plugins, no vim. Open a terminal pane, jot something down, close it.

```
cargo install --path .
tinynote
```

## How it works

- One note on screen at a time. **^K** opens the palette — fuzzy search across every note's title and body, plus the commands (new, delete, preview, quit). Type, arrow, enter.
- **^N** creates a note; the file is named after its first line and renamed as the title changes (`# Groceries` → `groceries.md`).
- **^P** flips to rendered markdown (headings, bold/italic, lists, checkboxes, quotes, code). Any of esc/enter/`e` flips back.
- Deleting asks first, inline. Enter confirms, esc cancels.
- Autosaves half a second after you stop typing, and on switch/quit.
- Mouse works where you'd expect: scroll the note or preview, click a palette row to open it, click outside to dismiss.

Because the notes are just files, everything else in your toolbox works on them too — `grep`, `cat`, git, Obsidian pointed at the same folder.

Set `TINYNOTE_DIR` to use a different notes directory.

## Keys

| Key | Action |
| --- | --- |
| `^K` | Palette: search notes, run commands |
| `^N` | New note |
| `^P` | Toggle markdown preview |
| `Esc` | Close palette / cancel / leave preview |
| `^Q` | Quit |

## Development

```
cargo run      # against ~/notes (set TINYNOTE_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm, tui-textarea for editing, pulldown-cmark for the preview renderer (`src/render.rs` — the piece that will grow into live preview).

## License

MIT
