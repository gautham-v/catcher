# tinynote

https://github.com/user-attachments/assets/5a060469-40c5-4fd8-a17c-cab2e60b6f96

A minimal note-taking app for the terminal. Local-first, no accounts, no sync — your notes are just a flat folder of `.md` files in `~/tinynote`, so `grep`, git, and Obsidian all work on them too.

Open a terminal pane, jot something down, close it.

## Install

```
brew install tinycomputer-io/tap/tinynote
```

Or with cargo:

```
cargo install tinynote
```

Then run `tinynote`.

## Using it

One note on screen at a time. **^K** opens the palette — fuzzy search across every note's title and body, plus commands (new, delete, rename file, preview, shortcuts, settings, quit).

The editor is a **live preview**, Obsidian-style: headings, emphasis, `==highlight==`, code, links, quotes, bullets and `☐`/`✓` checkboxes are styled as you type, and the line the cursor is on shows its raw markdown so it's always editable. Code fences, tables, rules and images are drawn whole and flip back to source when the cursor lands inside. Long lines soft-wrap; nothing scrolls sideways. **^P** flips to the full rendered page, with images drawn inline in terminals that support graphics (Ghostty, kitty, iTerm2).

Notes autosave half a second after you stop typing. Mouse works as expected: click to place the cursor, drag to select (copies on release), scroll, click a palette row. **^V** pastes — a clipboard image becomes a PNG in your attachments folder with the markdown link inserted for you.

**^N** creates a note. Its filename follows the first line (`# Groceries` → `groceries.md`) until you rename the file yourself, after which the title and filename go their own ways.

## CLI

```
tinynote                  open the TUI on the most recent note
tinynote groceries        open the note whose title best matches; create it if none does
tinynote add "buy milk"   write a new note and print its path — no TUI
cat x | tinynote add      same, from stdin
tinynote path             print the notes directory
tinynote ~/vault/spec.md  open that file, with the session rooted at its folder
tinynote ~/vault          open the TUI rooted at that folder
tinynote --keys           print the key events your terminal sends (esc quits)
tinynote --help           usage
```

Pointing tinynote at a file or folder roots that one session there without touching your config. Outside your configured notes dir, filenames are never auto-renamed (Obsidian links depend on them) and image paste is refused.

## Keys

| Key | Action |
| --- | --- |
| `^K` | Palette: search notes, run commands |
| `^G` | Keyboard shortcuts card |
| `^N` | New note |
| `^P` | Toggle markdown preview |
| `^S` | Save now |
| `^Z` / `^Y` | Undo / redo |
| `^C` `^X` `^V` | Copy / cut / paste |
| `Esc` | Close palette, cancel, leave preview, clear selection |
| `^Q` | Quit |

Editing is macOS-style — `⌘←`/`⌘→` for line start/end, `⌘↑`/`⌘↓` for top/bottom, `⌥←`/`⌥→` by word, `⌘⌫` and `⌥⌫` to delete, `⌘A` to select all. Add `⇧` to any movement to extend the selection. `^`-click or `⌥`-click follows a link.

If a shortcut misbehaves, run `tinynote --keys` to see what your terminal actually sends. Ghostty binds most Mac editing keys to legacy control codes, which tinynote handles — except `⌘↑`/`⌘↓`, which Ghostty keeps for itself. Use `^Home`/`^End`, or unbind them:

```
keybind = cmd+up=unbind
keybind = cmd+down=unbind
```

`⌥`-click on a link needs `macos-option-as-alt` set; `^`-click always works.

## Configuration

`~/.config/tinynote/config.toml` is written with commented defaults on first run:

```toml
notes_dir = "~/tinynote"                    # where the .md files live
attachments_dir = "~/tinynote/attachments"  # where pasted images are written
```

Both accept a leading `~/`. `TINYNOTE_DIR` overrides `notes_dir` for a single run: `TINYNOTE_DIR=/tmp/notes tinynote`. The palette's **Open settings** opens the file in `$EDITOR` and reloads on exit.

## Development

```
cargo run      # against ~/tinynote (set TINYNOTE_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm. `src/editor.rs` is the text buffer, `src/md.rs` the live-preview styling and click mapping, `src/render.rs` the full-page preview, `src/images.rs` inline images, `src/cli.rs` argument parsing, `src/config.rs` the config file, `src/clipboard.rs` copy and paste.

## License

MIT
