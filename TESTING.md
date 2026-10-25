# Testing mujo

Thanks for running this. Report by terminal, case, and a screenshot rather than
a description — a picture of a broken render is worth a paragraph.

## Setup

- macOS: apply the Terminal.app mappings from the README first
  (⇧↑ to `\033[1;2A`, ⇧↓ to `\033[1;2B`)
- You need `pass`, `gnupg`, and an initialised store with at least one entry
- A phone, for the QR scan cases
- A prebuilt binary is attached to the release; or build it with
  `xcode-select --install`, `rustup`, then `cargo build --release --locked`

## Cases

**Navigation** — move up/down, search `/`, new `n`, delete a file, delete a
folder, move `m`, add folder `f`. Confirm deleting a folder does not remove a
same-named file.

**Preview** — `r` reveal/hide, `c` copy, `w` QR. With the QR up: `w` closes,
Esc returns to the decrypted preview (not cleared), `r` returns and reveals,
`c` returns and copies. Scan the QR with a phone.

**Editor** — `e` on an entry, edit, `Ctrl+G` generate, `Ctrl+U`/`Ctrl+D`
length, Tab charset, `Ctrl+S` save, Esc discard.

**Git pane** — `g`, status, `p` pull, `P` push, Shift+↑↓ scroll the log.

**Help** — `?`, `Ctrl+/` and F1 from Normal, Preview, Editor and Git. Closing
must return you to the mode you came from.

**Keyboard** — run once with `--keyboard mac` and once with no flag; both should
advertise `Ctrl+U/D`, not `Ctrl+arrows`.

**Glyphs** — bmp (default), nerd, ascii. Text should stay legible in each.

**macOS specific** — ⇧↑/⇧↓ in the editor and preview, QR scan, and no vertical
smearing on bold blocks.

## Known limitations

- Long passwords use a denser QR render; it scans but looks coarser on
  Terminal.app
- Ctrl+arrows are captured by Mission Control, hence the Ctrl+U/D alternatives
