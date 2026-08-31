# tinynote

Raycast Notes in your terminal. A tiny markdown notes TUI over plain files.

Your notes are a flat folder of `.md` files in `~/notes` — nothing else. No folders, no tags, no sync, no accounts, no plugins, no vim. Open a terminal pane, jot something down, close it.

```
cargo install --path .
tinynote
```

## CLI

```
tinynote                  open the TUI on the most recent note
tinynote groceries        open the note whose title best matches; create it if none does
tinynote add "buy milk"   write a new note and print its path — no TUI
cat x | tinynote add      same, from stdin
tinynote path             print the notes directory
tinynote --keys           print the key events your terminal sends (esc quits)
tinynote ~/vault/spec.md  open that file, with the session rooted at its folder
tinynote ~/vault          open the TUI rooted at that folder
tinynote --help           usage
```

`tinynote <name>` fuzzy-matches note **titles** only, so it either lands on the note you meant or starts a new one with `# <Name>` at the top. `add` is the capture path: the first line becomes the title and so the filename, collision-safe, and the path goes to stdout for scripting.

Pointing tinynote at a file or a folder roots that one session there, as a per-invocation `TINYNOTE_DIR` — the config file is untouched, and the palette browses that folder's `.md` files (non-recursive). Outside your configured notes dir two things are deliberately off: filenames are **never** auto-renamed (an Obsidian vault's links depend on them) and pasting an image is refused with a flash, since attachments belong next to your own notes. Text paste still works.

## How it works

- One note on screen at a time. **^K** opens the palette — fuzzy search across every note's title and body, plus the commands (new, delete, rename file, preview, settings, quit). Type, arrow, enter.
- **^N** creates a note; the file is named after its first line and follows the title as it changes (`# Groceries` → `groceries.md`).
- The filename is only *tracking* the title while it still equals the slug of it. Rename the file yourself — the palette's **Rename file** opens a small inline prompt on the current stem, or just `mv` it in a shell — and the two are detached: from then on the title can change all it likes and the file stays put. Nothing is recorded to make that work; the check is the name against the note's own heading. Once they are detached the palette lists the note as `Hello World (hello.md)`, so it's clear the file went its own way. The status bar always shows the filename and only the filename — the title is already the first line of the note on screen, but the file it is being written to isn't visible anywhere else.
- The editor is a **live preview**, Obsidian-style: headings, emphasis, `==highlight==`, code, links, quote bars, bullets and `☐`/`✓` checkboxes are styled in place while you type. The line the cursor is on shows its raw markdown, so the syntax is always there to edit.
- The live preview is **block-aware**: fenced code blocks, tables, `---` rules and image lines are drawn whole — a table in aligned columns, a rule across the page, an image as the actual picture — and the moment the cursor (or a selection) lands anywhere inside one, that whole block flips back to raw source so it can be edited. A fence's own backticks are never on screen while the cursor is elsewhere: the opening line is just the dim language name, the closing line is blank, and clicking either still lands on it and opens the block up.
- Long lines **soft-wrap** in both views — nothing ever runs off the right edge, and there is no horizontal scroll. A wrapped list item or quote hangs its continuation rows under the text rather than under the marker. `↑`/`↓` move by the row you can see, the way they do everywhere else; `⌘←`/`⌘→` (`^A`/`^E`) still go to the real start and end of the line.
- **^P** flips to the full rendered page: tables laid out in aligned columns, `==highlight==`, `☐`/`✓` tasks, and images drawn inline when the terminal speaks kitty/iTerm2 graphics (Ghostty does), with a `🖼 alt (path)` line when it doesn't. Any of esc/enter/`e` flips back; arrows and PageUp/PageDown scroll.
- Both views follow links. In the preview a plain click on a link opens it; in the editor, where a click has to keep meaning "put the cursor here", it takes a modifier — `^`-click or `⌥`-click (see [Keys](#keys)).
- The preview is clickable: a link opens in your browser, a checkbox toggles in the file, and a click anywhere else drops you into the editor at that spot.
- Deleting asks first, inline. Enter confirms, esc cancels.
- Autosaves half a second after you stop typing, and on switch/quit.
- Mouse works where you'd expect: click to place the cursor (styling and hidden markers are mapped back to the source), drag to select, scroll the note or preview, click a palette row to open it, click outside to dismiss.
- Selecting with the mouse copies to the system clipboard on release (OSC 52, with a `pbcopy` fallback); **^C** copies the selection too.
- **^V** pastes. If the clipboard holds an image (a screenshot, a copied picture) it's written as a PNG into your attachments folder — named after the note, `groceries-1.png` — and `![](attachments/groceries-1.png)` lands at the cursor, so it shows up inline in the preview straight away. Otherwise the clipboard text is pasted. Anything that goes wrong is a status-bar flash, never a crash.

Because the notes are just files, everything else in your toolbox works on them too — `grep`, `cat`, git, Obsidian pointed at the same folder.

## Configuration

`~/.config/tinynote/config.toml` is written with commented defaults the first time you run tinynote. Two keys:

```toml
notes_dir = "~/notes"                    # where the .md files live
attachments_dir = "~/notes/attachments"  # where pasted images are written
```

`attachments_dir` defaults to `<notes_dir>/attachments`, and both accept a leading `~/`. `TINYNOTE_DIR` still overrides `notes_dir`, which is handy for a scratch folder: `TINYNOTE_DIR=/tmp/notes tinynote`.

The palette's **Open settings** suspends the TUI, opens the file in `$VISUAL`/`$EDITOR` (`vi` if neither is set), and reloads on exit. A changed `attachments_dir` takes effect immediately; a changed `notes_dir` asks you to restart.

## Keys

| Key | Action |
| --- | --- |
| `^K` | Palette: search notes, run commands |
| `^N` | New note |
| `^P` | Toggle markdown preview |
| `^C` | Copy selection |
| `^V` | Paste — clipboard image as an attachment, else text |
| `Esc` | Close palette / cancel / leave preview / clear selection |
| `Enter` / `e` | Leave the preview (in preview only) |
| `↑` `↓` `PgUp` `PgDn` | Scroll the preview; move the cursor in the editor |
| `^Q` | Quit |

Editing, macOS-style. Add `⇧` to any of the movements to extend the selection.

| Key | Action |
| --- | --- |
| `⌘←` / `⌘→` | Start / end of line (`Home` / `End` do the same) |
| `⌘↑` / `⌘↓` | Top / bottom of the note |
| `⌥←` / `⌥→` | Word left / right (`^←` / `^→` too) |
| `⌘⌫` | Delete to the start of the line |
| `⌥⌫` | Delete the previous word |
| `^`-click / `⌥`-click | Follow the link under the cursor (editor) |

These work in two quite different ways, and it is worth knowing which.

tinynote pushes the [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) at startup where `supports_keyboard_enhancement()` says the terminal speaks it (Ghostty, kitty, WezTerm, foot), and pops it again on exit, on a panic, and around the `$EDITOR` suspend. With it, `⌘` arrives as a real Super modifier on the arrow keys and the shifted movements survive.

**But in Ghostty the protocol never sees the Mac editing keys.** Ghostty's default macOS config binds them to legacy bytes, and a keybind is resolved before the key is ever encoded, kitty protocol or not. So what actually arrives is:

| Key | Ghostty's default | tinynote sees |
| --- | --- | --- |
| `⌘←` / `⌘→` | `text = "\x01"` / `"\x05"` | `^A` / `^E` |
| `⌘⌫` | `text = "\x15"` | `^U` |
| `⌥←` / `⌥→` | `esc = "b"` / `"f"` | `⌥b` / `⌥f` |
| `⌥⌫` | unbound; the encoder sends `ESC DEL` | `⌥⌫` |
| `⌘↑` / `⌘↓` | `jump_to_prompt` — an app action | **nothing at all** |

tinynote handles every one of those legacy chords, so the whole table above works in Ghostty out of the box — they are the readline chords besides, so `^A`/`^E`/`^U`/`^W` are worth having anyway. `⌘↑`/`⌘↓` are the exception: Ghostty keeps them for jumping around its own scrollback and nothing reaches the pty, so `^Home` / `^End` are the stand-ins. To get the real keys, unbind them in `~/.config/ghostty/config` — with the kitty protocol pushed they then arrive as `⌘` proper:

```
keybind = cmd+up=unbind
keybind = cmd+down=unbind
```

`macos-option-as-alt` doesn't come into it for `⌥`-arrows or `⌥⌫` (they carry no text, so the Alt bit is reported either way), but it *is* needed for `⌥`-click on a link.

Where none of this is available, `Home`/`End`/`PgUp`/`PgDn` and `⌥`/`^`-arrows still work — most terminals send those natively.

### Troubleshooting keys

`tinynote --keys` enters raw mode, pushes the same keyboard enhancement the TUI does, and prints every key event — code, modifiers, kind — until you press Esc. If a shortcut isn't doing what this table says, run it and see what your terminal is really sending:

```
$ tinynote --keys
keyboard enhancement: supported=Ok(true) pushed=true
press keys — esc to quit
code=Char('a')  modifiers=KeyModifiers(CONTROL)  kind=Press  state=KeyEventState(0x0)
```

**Following links** is `^`-click or `⌥`-click, not `⌘`-click: SGR mouse reporting carries only shift, alt and ctrl bits, so no terminal can tell tinynote that Cmd was held on a click — Ghostty included. (`⌘`-click is honoured too, should a terminal ever report it.) Click anywhere on a `[text](url)` span or on a bare `http(s)://` URL and it opens in your browser; a plain click still just places the cursor, and image links are left alone. In Ghostty, `⌥`-click needs `macos-option-as-alt` set; `^`-click always works.

## Development

```
cargo run      # against ~/notes (set TINYNOTE_DIR to test elsewhere)
cargo test
cargo clippy
```

Rust, ratatui + crossterm. `src/editor.rs` is the text buffer, `src/md.rs` the line-based markdown styling and the click → source-column mapping shared by both views, `src/render.rs` the pulldown-cmark full-page preview (styled cells, link spans and checkbox lines), `src/images.rs` the inline-image protocol handling, `src/cli.rs` the hand-rolled argument parsing, `src/config.rs` the config file, `src/clipboard.rs` copy and paste (arboard, with OSC 52 and `pbcopy`/`pbpaste` fallbacks).

## License

MIT
