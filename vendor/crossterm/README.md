crossterm 0.28.1 from crates.io, patched in one place for catcher: the mouse
parser in `src/event/sys/unix/parse.rs` (`parse_cb`) passes the side buttons
(8 back, 9 forward) through instead of dropping them. Wired in with
`[patch.crates-io]` in catcher's Cargo.toml; drop this copy once upstream
reports those buttons.
