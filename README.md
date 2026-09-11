# catcher

A minimal note-taking app for the terminal. Local-first, no accounts, no sync: your notes are a folder of `.md` files in `~/catcher`, so `grep`, git and Obsidian work on them too.

![catcher's reading view: a note with a TL;DR callout, a table and a mermaid flowchart, over Sanford Gifford's A Gorge in the Mountains](docs/hero.jpg)

<sub>Background: Sanford Robinson Gifford, <i>A Gorge in the Mountains (Kauterskill Clove)</i>, 1862. The Metropolitan Museum of Art, public domain.</sub>

## Install

```
brew install gautham-v/tap/catcher
```

or `cargo install catcher`. Then run `catcher`.

## What it does

- **Live-preview editor.** Markdown renders as you type; markup shows only where the cursor is. **^P** flips to a reading view.
- **Obsidian-compatible.** `[[wikilinks]]`, embeds, `#tags`, callouts, front matter, aliases, daily notes, templates, bookmarks and `.trash` all work the Obsidian way. Point it at an existing vault.
- **Links stay intact.** Renaming a note or heading, or merging one note into another, rewrites every link to it.
- **Fast navigation.** **^O** fuzzy-opens notes, **⇧^F** searches the vault with Obsidian's operators, **⌥P** peeks at a link, **⌥click** opens it in a terminal split (Ghostty, tmux, kitty, WezTerm).
- **Rich rendering.** Inline images (Ghostty, kitty, iTerm2), tables, syntax-highlighted code, foldable sections, and mermaid diagrams drawn in text.
- **Plain files, safe edits.** Autosave, reload on external change with undo, and deletes go to `.trash`.

**^K** opens the command palette; everything is in there.

## CLI

```
catcher                  open the note you last had open
catcher groceries        open the note whose title best matches
catcher new groceries    create a note and open it
catcher today            open today's journal note
catcher add "buy milk"   write a new note and print its path
catcher ~/vault          open the TUI rooted at that folder
catcher path             print the notes directory
```

## Keys

| Key | Action |
| --- | --- |
| `^K` | Command palette |
| `^O` | Open a note |
| `^F` / `⇧^F` | Find in note / search all files |
| `^N` | New note |
| `⌥D` | Today's note |
| `^P` | Reading view |
| `⌥⏎` / `⌥P` | Follow / peek at a link |
| `⌥[` / `⌥]` | Back / forward |
| `^/` | Help card |
| `^,` | Settings |
| `^Q` | Quit |

Editing is macOS-style. Every key is rebindable.

## Settings

**^,** opens `~/.config/catcher/settings.md` as a note; **^S** applies it. Each setting has a hint beside it. `CATCHER_DIR` overrides the notes folder.

Called **tinynote** until 0.9; existing `~/tinynote` and `~/.config/tinynote` folders are still picked up.

## Development

```
cargo run      # against ~/catcher (set CATCHER_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm.

## License

MIT
