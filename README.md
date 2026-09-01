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

A bare `tinynote` reopens the note you had open when you closed it, wherever it lives. Naming something — `tinynote spec.md`, `tinynote ~/vault` — asks for that instead.

One note on screen at a time. **^K** opens the palette — every note listed by filename, fuzzy-searched by filename first, then title, then body, plus commands (new, open, delete, rename file, preview, help, settings, quit).

**^O** opens a note. It is the palette's twin, and the difference is what it ranks by: notes you opened most recently first, then the most recently edited, and it walks *subfolders* — so pointing tinynote at an Obsidian vault still lets you jump straight to `applications/log.md` from wherever you are. Both the list and the tree show each note by its filename, the name a `[[wikilink]]` reaches it by. Type to fuzzy-search filenames, with titles and folder paths as a second chance.

**Tab** flips **^O** between the ranked list and a folder tree — the same index, the other way of looking at it. The tree opens on the note you have open, with the folders above it unfolded, so the first thing it tells you is where you are; **→** unfolds a folder or opens a note, **←** folds one or steps out to the folder it lives in, and a closed folder says how many notes are beneath it. Whatever you have typed comes along either way, so `log` and then tab shows you *where* the log notes live. `quick_open_mode: browse` in the settings makes the tree what **^O** opens on.

It reaches past your notes dir three ways. A note **you have opened before** is always offered, wherever it lives — that is what the recents list is for, and it survives restarts. Folders listed in `quick_open_dirs` are searched every time. And typing a **path** — `~/vault/spec.md`, or `~/vault/spec` — opens that file directly, which is the escape hatch for a note tinynote has never been shown. Opening a note from another folder pulls it into the session; it saves back where it lives, and is never renamed.

The editor is a **live preview**, Obsidian-style: headings, emphasis, `==highlight==`, code, links, quotes, bullets and `☐`/`✓` checkboxes are styled as you type, and the line the cursor is on shows its raw markdown so it's always editable. Code fences, tables, rules and images are drawn whole and flip back to source when the cursor lands inside. Long lines soft-wrap; nothing scrolls sideways. **^P** flips to the full rendered page, with images drawn inline in terminals that support graphics (Ghostty, kitty, iTerm2). The view sticks across notes: follow a link, pick a linked mention or open another note while reading and it opens as a page too; a new note always opens in the editor. Quotes keep their `▌` rail down every row, blank and wrapped ones included, and an Obsidian callout (`> [!summary] TL;DR`, or `[!note]`, `[!tip]`, `[!warning]`…) is drawn as a boxed card with its type and title across the top.

`[[wikilinks]]` work the way an Obsidian vault expects. `[[spec]]` finds the note by filename, title, or the tail of its path — `[[stories/story-matrix]]` — and `[[spec|the spec]]` draws the label instead. `⌥⏎` on one follows it, and so does `^`-click or `⌥`-click; a link that names no note is drawn in the danger colour, and following it offers to create the note, in the folder the target named. `[[#heading]]`, `![[image.png]]` and `\[[escaped]]` are left alone, and so is a link inside a code fence. `wikilinks: no` leaves them all as the literal text a reader without Obsidian sees.

A `---` front matter block is metadata rather than prose: the reading view never shows it, and `front_matter` in the settings says what the editor does with it — `dim` it, `show` it styled like any other markdown, or `hide` it until the cursor moves in. Adding `properties` to `status_bar_items` puts a count of the keys it declares in the bottom line, on the notes that have any.

At the foot of the reading view is **linked mentions**: the notes that link to the one you are reading, each with the sentence the link sits in, most recently edited first. A note that links here several times is one row with a `×3` beside it, and every row is itself a link — follow it to go there. A note nothing points at gets no footer at all, not even a rule. It is a scan and not a stored graph — nothing is kept beside your notes — so it runs on a thread and the footer appears a moment after the page does. `linked_mentions: no` turns it off, and so does turning `wikilinks` off, since there would be nothing to count.

In the reading view a wide table is not squeezed into columns two characters across. Its columns keep a readable width and wrap inside it, and the table itself pans sideways — **←** and **→**, or a sideways scroll — with a `›` on the header row where it carries on. Nothing is cut. `table_style` in the settings picks the rule: `auto` (leave a table that fits alone, scroll one that doesn't), `scroll`, `fit`, `wrap`, or `cards` for one labelled block per row.

Notes autosave half a second after you stop typing. Mouse works as expected: click to place the cursor, drag to select (copies on release), scroll, click a palette row. In the reading view a click no longer drops you into the editor — drag to select and it copies on release, so you can lift a quote out of a rendered page. **^V** pastes — a clipboard image becomes a PNG in your attachments folder with the markdown link inserted for you.

**^N** creates a note. Its filename follows the first line (`# Groceries` → `groceries.md`) until you rename the file yourself, after which the title and filename go their own ways.

## CLI

```
tinynote                  open the TUI on the note you last had open
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
| `^O` | Open a note: every folder, recent first |
| `^/` or `F1` | Help card: every key |
| `^N` | New note |
| `^,` | Settings |
| `^P` | Toggle markdown preview |
| `^S` | Save now |
| `^Z` / `^Y` | Undo / redo |
| `^C` `^X` `^V` | Copy / cut / paste |
| `⌥⏎` | Follow the `[[wikilink]]` under the cursor |
| `⌥P` | Peek at the `[[wikilink]]` under the cursor; in preview, hovering a link does the same |
| `^B` / `^F` | Back / forward through the notes you have opened, browser-style |
| `Tab` | In **^O**: flip between the ranked list and the folder tree |
| `Esc` | Close palette, cancel, leave preview, clear selection |
| `^Q` | Quit |

Every key in that table but `Tab` is settable — tab means “the other view of this” and is not a command. The settings note has a `## Keys` section with one line per action — `key_palette: ^K`, `key_open: ^O` — and takes `^K`, `cmd+k`, `alt+k`, `f5`, or `none` to unbind — or several at once, as `^/ f1`. `^K` answers to either ctrl or cmd, so the same file works on a Mac and on Linux; spell out `cmd+` or `ctrl+` when you want one exactly. **Delete note** and **Rename file** ship unbound and are yours to claim. The palette shows each command's current key beside it, and so does the **^/** help card (`F1` opens it too, for terminals that swallow `^/`) — which is itself searchable: type `save` on it and only the saving rows stay. Esc closes it. The palette itself is monochrome — it is chrome over the note, and a hue there would compete with the one the note spends on its headings.

The palette's search box takes the Mac editing keys too: `⌘⌫` clears it, `⌥⌫` deletes a word.

Editing is macOS-style — `⌘←`/`⌘→` for line start/end, `⌘↑`/`⌘↓` for top/bottom, `⌥←`/`⌥→` by word, `⌘⌫` and `⌥⌫` to delete, `⌘A` to select all. Add `⇧` to any movement to extend the selection. `^`-click or `⌥`-click follows a link.

If a shortcut misbehaves, run `tinynote --keys` to see what your terminal actually sends. Ghostty binds most Mac editing keys to legacy control codes, which tinynote handles — except `⌘↑`/`⌘↓`, which Ghostty keeps for itself. Use `^Home`/`^End`, or unbind them:

```
keybind = cmd+up=unbind
keybind = cmd+down=unbind
```

`⌥`-click on a link needs `macos-option-as-alt` set; `^`-click always works.

## Settings

Every note you land on — from **^O**, the tree, a `[[wikilink]]`, a linked mention, **^N** — goes on a history, and `^B` / `^F` walk it the way a browser does: going back and then opening something else drops what was ahead, and a note deleted in the meantime is skipped.

Settings are a note. **^,** opens `~/.config/tinynote/settings.md` in tinynote itself — same editor, same preview, no `$EDITOR` and no TOML — and **^S** applies it at once. Colours, page width and everything else but `notes_dir` change on the next frame.

Every setting is a `- key: value` line with a one-line hint after it. The file is written on first run with all of them in it, and rewritten when a new setting appears — your values are kept, since the file is generated from the settings it was just read into.

| | |
| --- | --- |
| `notes_dir`, `attachments_dir` | where notes and pasted images live (`~/` expands; `TINYNOTE_DIR` overrides the first) |
| `theme` | `dark` or `light` — which way your terminal's own background runs |
| `accent`, `bright`, `grey`, `dim`, `link`, `code_bg`, `code_fg`, `border`, `danger`, `ground` | the ten colours, as `#rrggbb`, `#rgb`, an ANSI name, `default`, or `theme` to leave it to the theme |
| `page_width` | widest the note column is drawn, in columns, or `full` |
| `borders` | `rounded`, `square`, `none` |
| `bold_headings`, `status_bar`, `key_hints` | chrome, on or off |
| `status_bar_items` | what the bottom line shows, in order: `path`, `name`, `mode`, `properties`, `keys`, `message` |
| `autosave_ms`, `tab_width` | how soon a note saves, how far `tab` goes |
| `rename_files` | whether a filename follows its note's title |
| `front_matter` | `dim`, `show`, `hide` — what the editor does with a `---` block (the reading view never shows one) |
| `table_style` | `auto`, `scroll`, `fit`, `wrap`, `cards` — what happens to a table wider than the page |
| `preview_click` | `select` or `edit` — what a click in the reading view does |
| `wikilinks` | whether `[[links]]` open notes, or stay the literal text |
| `linked_mentions` | the notes that link here, at the foot of the reading view |
| `quick_open` | `recursive` or `folder` — how far **^O** looks |
| `quick_open_mode` | `search` or `browse` — which half of **^O** it opens on |
| `quick_open_dirs` | extra folders **^O** searches; repeat the line, or separate with commas |
| `key_palette`, `key_open`, `key_new`, `key_settings`, `key_preview`, `key_save`, `key_help` (`key_shortcuts` still read), `key_quit`, `key_copy`, `key_cut`, `key_paste`, `key_undo`, `key_redo`, `key_delete`, `key_rename`, `key_follow`, `key_back`, `key_forward`, `key_peek` | one key each — `^K`, `cmd+k`, `alt+k`, `f5`, or `none` |

An existing `config.toml` is read once, to seed `settings.md`, and then left alone.

## Development

```
cargo run      # against ~/tinynote (set TINYNOTE_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm. `src/editor.rs` is the text buffer, `src/md.rs` the live-preview styling and click mapping, `src/render.rs` the full-page preview, `src/images.rs` inline images, `src/cli.rs` argument parsing, `src/config.rs` the settings note, `src/keys.rs` the bindings, `src/index.rs` the quick-open index and recents, `src/tree.rs` the folder tree behind **^O**'s browse mode, `src/mentions.rs` the linked-mentions scan, `src/clipboard.rs` copy and paste. Every colour in the app lives in one place: the `theme` module at the top of `src/md.rs`.

## License

MIT
