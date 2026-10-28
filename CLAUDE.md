# catcher

Minimal, local-first markdown notes TUI over plain files, written in Rust.

## Releasing

A release goes to all five channels, in this order. Never stop after the GitHub release: `cargo install catcher` users only see the crates.io version, and releases 0.16 to 0.17 skipped it so crates.io lagged behind GitHub and Homebrew.

1. Bump `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, commit both as `vX.Y.Z`, and push `main`.
2. `gh release create vX.Y.Z --title "vX.Y.Z — <lowercase summary>" --notes-file <path>`. Notes are for app users: what changed on the page, which keys moved, in present-tense bullets with no file or function names.
3. Update `Formula/catcher.rb` in the `gautham-v/homebrew-tap` repo: the tag tarball `url` and its `sha256`. Check the tarball download returns 200 before trusting the hash, then commit as `catcher vX.Y.Z` and push.
4. `cargo publish --dry-run && cargo publish`. Delete `.DS_Store` files first; publish refuses them.
5. Report the version each channel now shows.

Patch version for a fix, minor for new behavior or a changed key binding.
