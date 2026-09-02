# catcher

https://github.com/user-attachments/assets/5a060469-40c5-4fd8-a17c-cab2e60b6f96

A minimal note-taking app for the terminal. Local-first, no accounts, no sync: your notes are a folder of `.md` files in `~/catcher`, so `grep`, git and Obsidian work on them too.

> Called **tinynote** until 0.9. Existing `~/tinynote` and `~/.config/tinynote` folders are still picked up.

## Install

```
brew install tinycomputer-io/tap/catcher
```

or `cargo install catcher`. Then run `catcher`.

## Using it

- **^K** is the command palette: new, open, delete, rename, move to folder, reading view, help, settings, quit.
- **^O** opens a note: every folder, most recently opened first, fuzzy-searched by filename. **Tab** steps through its tabs: recent, a folder tree, and contents.
- **⇧^F** searches in all files: every line that has every word you type, grouped by note. **⏎** opens the note at that line.
- **^N** makes a note. The filename follows its first line until you rename the file yourself. Either way, `[[links]]` to the old name in other notes are rewritten to the new one.
- **⌥D** opens today's note, `journal/2026-09-01.md`, made from `journal/template.md` the first time (`{{title}}`, `{{date}}`, `{{yesterday}}`, `{{tomorrow}}`) and never rewritten after.
- The palette also has editing commands, unbound until you give them a key: toggle checkbox (`- item` → `- [ ]` → `- [x]` → `- item`, numbered lists too), move line up / down, toggle heading (`#`, `##`, `###`, none), insert today's date, copy path, reveal in Finder. Each takes the selection when there is one, and undoes as one step.
- **^P** flips between the live-preview editor and the rendered page. Images draw inline in Ghostty, kitty and iTerm2.
- `[[wikilinks]]` work like Obsidian's. **⌥⏎** follows one, **⌥P** peeks at it, **^B** / **^F** go back and forward. A link to a note that does not exist yet is grey; following it makes the note beside the one you are in. The reading view lists the notes that link here at the bottom.
- `#tags` are coloured, inline or as front matter `tags:`. **⌥⏎** or a click on one opens **^O** cut to the notes that carry it.
- A ` ```mermaid ` fence is drawn as text: flowcharts and sequence diagrams, in box-drawing characters, no images and no network. Other kinds keep their source under a label.
- Callouts, front matter, tables wider than the page (they pan sideways), checkboxes and `==highlights==` all render.
- Notes autosave. Edit a note in another program and catcher takes what is on disk: the buffer reloads, the cursor stays on its line, and **^Z** brings the old buffer back. A file deleted from under it is said so in the status bar and recreated on the next save.
- Mouse works: click, drag to select and copy, scroll. **^V** pastes a clipboard image as a PNG.

## CLI

```
catcher                  open the note you last had open
catcher groceries        open the note that best matches; create it if none does
catcher today            open today's note in `journal/`, creating it from the template
catcher add "buy milk"   write a new note and print its path
catcher ~/vault          open the TUI rooted at that folder
catcher path             print the notes directory
catcher --keys           show what your terminal sends for each key
```

## Keys

| Key | Action |
| --- | --- |
| `^K` | Command palette |
| `^O` | Open a note (`Tab` for the folder tree, again for contents) |
| `⇧^F` | Search in all files |
| `^N` | New note |
| `⌥D` | Today's note |
| `^P` | Reading view |
| `^S` | Save now |
| `^Z` / `^Y` | Undo / redo |
| `^C` `^X` `^V` | Copy / cut / paste |
| `⌥⏎` / `⌥P` | Follow / peek at the `[[wikilink]]` under the cursor (a missing note is created; `⌥⏎` follows a `#tag` too) |
| `^B` / `^F` | Back / forward |
| `^/` or `F1` | Help card |
| `^,` | Settings |
| `^Q` | Quit |

Editing is macOS-style: `⌘←`/`⌘→` line ends, `⌥←`/`⌥→` by word, `⇧` to extend a selection, `⌘A` select all. Every key is rebindable in the settings; `^K` answers to ctrl or cmd. `⇧^F` needs a terminal that tells shift+ctrl apart (Ghostty, kitty, WezTerm); elsewhere it arrives as `^F` and the palette is the way in. The editing commands ship with no key: `key_checkbox`, `key_line_up`, `key_line_down`, `key_heading`, `key_date`, `key_copy_path`, `key_reveal` bind them.

## Settings

**^,** opens `~/.config/catcher/settings.md` as a note; **^S** applies it. It covers the notes and attachments folders, the daily note's folder and template, theme (`auto`, `dark`, `light`) and colours, page width, borders, status bar, autosave delay, tab width, front matter, table style, wikilinks, whether a rename updates links, linked mentions, how far **^O** looks, and every key binding. Each line has a one-line hint beside it. `CATCHER_DIR` overrides the notes folder.

## Development

```
cargo run      # against ~/catcher (set CATCHER_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm. `src/editor.rs` is the buffer, `src/md.rs` live-preview styling, `src/render.rs` the reading view, `src/mermaid/` the diagram renderer, `src/config.rs` the settings note. Every colour lives in the `theme` module at the top of `src/md.rs`.

## License

MIT
