# Logbook

`docs/design.md` is the spec; the README is the tour. Read the spec section
for whatever you change.

## Crates

- `bar-anchor` is no longer in this repo: it's the shared
  [bar-anchor](https://github.com/NathanielInman/bar-anchor) crate, pinned by tag in the root `Cargo.toml`.
  Change it there and move the tag; its `Anchor` JSON must stay
  identical across the apps.
- `logbook-core`: paths, config, the model and lamp order (`model.rs`), the
  socket protocol, the recordings on disk (`library.rs`), the transcript plan
  (`plan.rs`), and the parsers: WAV and MP4 (`media.rs`), PipeWire's graph
  (`mic.rs`), voxtype's state (`voxtype.rs`), Switchyard's snapshot
  (`yard.rs`), waybar's CSS (`waybar.rs`). No processes, no PipeWire, no
  GTK.
- `logbook`: the daemon (`daemon.rs`), child ownership (`proc.rs`), the
  PipeWire thread (`pw.rs`), the voxtype watches on its state and history
  (`voxwatch.rs`), the transcribe worker (`transcribe.rs`), the CLI and the
  waybar line.
- `logbook-board`: the GTK4 layer-shell drawer. `anim.rs` and `stage.rs`
  came from Roundhouse's board by way of Tannoy.

## Build and test

`cargo build --workspace --locked`, `cargo test --workspace --locked` and
`cargo clippy --workspace --all-targets -- -D warnings`. The `pipewire`
crate needs libpipewire's headers and libclang; the drawer needs
gtk4-layer-shell. `.github/scripts/libraries.sh` is how CI gets them.

Parsers and planners are pure functions tested against plain data. Add a
test with a real sample when you touch one.

## Isolation

This machine may run an installed Logbook, and the mic is real. Point a dev
build elsewhere with `LOGBOOK_RUNTIME_DIR`, `LOGBOOK_STATE_DIR` and
`LOGBOOK_CONFIG_DIR` (keep the runtime dir short, such as
`$XDG_RUNTIME_DIR/lbdev`: the socket path must fit in 107 bytes), and give
it a config whose `meeting.dir` and `screen.dir` are scratch folders with
`dnd = false`. Set `XDG_DATA_HOME` to a
scratch folder with `voxtype/history/YYYY-MM-DD.jsonl` in it to give the
Dictations tab made-up history: the real `voxtype history list` reads it.
The drawer's Turn on, Pause and Resume write voxtype's real config; give
voxtype a scratch one with `XDG_CONFIG_HOME` before clicking them.

Never test recording against the real microphone. WirePlumber here ignores
`pw-record --target` for a virtual source and links the default mic anyway.
Put fake `pw-record`, `wf-recorder`, `slurp` and `ffmpeg` scripts first on
`PATH` instead: a `pw-record` that appends silence to a WAV and exits 0 on
SIGINT exercises chunking, marks, stop and crash recovery. Set
`transcribe.command` to a script that prints text and `wrap = []`.

`logbook-board --render out.png [tab]` draws the panel to a PNG. Run it
under `gtk4-broadwayd` (`GDK_BACKEND=broadway`) to see the drawer without
opening it on the desktop.

## Rules

- Every child the daemon starts goes through `proc::spawn` (or
  `proc::command` for short helpers): its own process group, a parent-death
  signal, and a clean signal mask. Never `Command::new` a long-lived child
  in the daemon.
- Never block the state lock on a process: stop recorders, call swaync,
  pactl or Switchyard outside it.
- Live state goes in `$XDG_RUNTIME_DIR`, never `/tmp`. Anything that must
  survive a reboot (the transcript queue) goes in the state dir.
- No polling. The ticker runs once a second only while something records,
  and Switchyard is asked about a transcript only while it holds one.
- Recording folders keep `meeting-record.sh`'s layout so old meetings still
  list and transcribe.
- Do not disturb that Logbook did not turn on, Logbook does not turn off.

## When behavior changes

Update in the same commit:

- `docs/design.md` for a new config key, protocol message, lamp state or
  rule.
- The README for anything a new user meets (CLI, keys, install, config).
- `docs/assets/drawer.png` when the drawer looks different (render it with
  `--render`, the board's `XDG_DATA_HOME` on a scratch folder so it draws in
  the fallback mono the hero uses, not your own fonts), and `drawer()` in
  `docs/assets/hero.py` to match (then run `python3 docs/assets/hero.py`).
  dot-files' bar tour ports that function.

Commits are conventional, scoped by area: `anchor`, `core`, `plan`, `mic`,
`daemon`, `cli`, `board`, `docs`, `ci`.

## Platforms

The suite targets Linux, Windows and macOS. `platforms` in
`docs/assets/suite.toml` lists where this app builds today, and `planned`
what a port still owes; CI and the pre-push hook hold it to `platforms`.

- Turn the hook on once per clone: `git config core.hooksPath .githooks`.
  Before a push it runs clippy for every platform listed (needs
  `rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin` and, for
  crates with C code, `mingw-w64-gcc`). A failure there is a broken build on
  that platform: fix it, don't push past it with `--no-verify`.
- Code for one platform goes behind `#[cfg(...)]`, helpers included: a
  function only Linux calls is dead code on Windows, and `-D warnings` fails
  the build there.
- A test that needs something one platform has (a Linux-only binary, Unix
  socket behavior, a POSIX shell, a path layout) is gated with `#[cfg(...)]`
  or skips elsewhere.
