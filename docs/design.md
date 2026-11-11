# Logbook design

Logbook is one lamp in waybar for everything that listens to the mic: meeting
recordings, screen captures, voxtype dictation, and any other app with a
capture stream open. Behind the lamp sits a drawer with the live recording,
the recordings on disk and their transcripts.

It replaces three scripts and three bar icons from my dot-files:
`meeting-record.sh`, `screen-record.sh` and `voxtype-status.sh`.

## Goals

1. **The lamp tells the truth.** It is dark only when nothing has a mic open.
   A recording that the bar says is stopped is stopped.
2. **Stop really stops.** The daemon owns every recorder it starts and ends
   the whole process group on stop, on its own exit and on its own crash.
3. **No stale state.** Live state sits in `$XDG_RUNTIME_DIR`, which a reboot
   clears. A daemon that finds live state from a crashed daemon cleans it up.
4. **Transcripts arrive on their own.** A stopped meeting queues its
   transcript, which waits its turn behind heavy builds in Switchyard.
5. **No polling.** PipeWire, voxtype's state file and the recorders report
   their own changes. The clock ticks once a second only while something
   records.

## Non-goals

- Editing audio or video. Logbook records, normalizes a capture's loudness,
  and hands files to whatever opens them.
- Its own speech model. The transcriber is a command (`voxtype transcribe` by
  default) that prints text for one WAV file.
- X11 or compositors without wlr-layer-shell. The drawer then opens as a
  plain window; nothing else is tested there.

## Architecture

```
logbook daemon ──owns──> pw-record (one per 15 minute chunk)
      │         ──owns──> wf-recorder, ffmpeg (loudness pass)
      │         ──owns──> lowprio logbook transcribe <dir>   (no Switchyard)
      │         ──run───> Switchyard task logbook-transcribe  (with Switchyard)
      │ <─registry── PipeWire (capture streams and their links)
      │ <─inotify─── $XDG_RUNTIME_DIR/voxtype/state, pidfd on voxtype's pid
      │
      ├── socket ──> logbook bar        (waybar custom module, JSON lines)
      ├── socket ──> logbook-board      (GTK4 layer-shell drawer)
      └── socket <── logbook CLI, logbook transcribe (progress reports)
```

Crates:

| Crate | What |
|---|---|
| `bar-anchor` | Hyprland anchoring and panel placement, copied from Roundhouse. |
| `logbook-core` | Paths, config, model, socket protocol, recordings on disk, the transcript plan, and the WAV, MP4, PipeWire-graph, voxtype and Switchyard parsers. No processes, no PipeWire, no GTK. |
| `logbook` | The daemon, the CLI, the waybar module and the transcribe worker. |
| `logbook-board` | The drawer. |

## Process ownership

Every child the daemon starts goes through `proc::spawn`:

- **Its own process group** (`setpgid(0, 0)`), so stop signals `-pgid` and
  reaches everything the child started. The old meeting script signalled a
  group that did not exist and left its chunk loop recording.
- **A parent-death signal** (`PR_SET_PDEATHSIG`): SIGINT for recorders, so a
  WAV or MP4 gets closed, SIGTERM for the rest. The kernel sends it when the
  thread that forked the child exits, so every child is forked on one owner
  thread that lives as long as the daemon.
- **A clean signal state.** The daemon blocks SIGTERM, SIGINT and SIGHUP in
  every thread so one thread can `sigwait` for them. A blocked mask survives
  exec, so each child unblocks them and resets their handlers before exec.
  Without that, pw-record ignores the SIGINT that asks it to finish its file.

Stopping a child sends the stop signal to the group, waits (3 seconds for
pw-record, 10 for wf-recorder, which writes the MP4 index on exit), then
SIGKILLs whatever is left in the group.

On SIGTERM, SIGINT or SIGHUP the daemon stops its recorders, gives back do
not disturb, saves the transcript queue, removes `live.json` and the socket,
and exits. A daemon killed with SIGKILL leaves `live.json` behind; its
recorders still die from the parent-death signal. The next daemon reads
`live.json`, ends any group that is still alive, turns do not disturb off if
the dead daemon had turned it on, and queues the meeting it was recording.

## Recordings

### Meetings

`pw-record --rate 16000 --channels 1 --format s16 [--target <source>]` into
`<meeting.dir>/<YYYY-MM-DD_HH-MM-SS>/chunk-NNN-HHMMSS.wav`, a new chunk every
`meeting.chunk` seconds (900). The next chunk starts before the previous one
is stopped, so no audio falls between them. This is the layout
`meeting-record.sh` used, so its folders list and transcribe too.

A pw-record that exits on its own gets replaced by the next chunk. Two that
die within 3 seconds of starting stop the meeting with a problem the drawer
shows.

**Marks.** `logbook mark` stores seconds since the start in the folder's
`marks.json` (`[{"at": 1872.4}]`). The transcript gets `## Mark 2 · 00:31:12`
at that moment.

### Screen

The CLI runs `slurp` for a region (the drawer closes first), then the daemon
runs `wf-recorder -y -g <region> --audio=<source> -f
<screen.dir>/recording-YYYYmmdd-HHMMSS.mp4`. `screen.audio = "monitor"` uses
`<pactl get-default-sink>.monitor`. On stop the lamp turns amber while
`ffmpeg -af loudnorm -c:v copy` writes a normalized copy, which replaces the
capture when it worked.

### Do not disturb

While a meeting (`meeting.dnd`) or screen capture (`screen.dnd`) records,
Logbook turns swaync's do not disturb on with `swaync-client -dn`, if it was
off, and off again with `-df` when the last such recording ends. Do not
disturb that was already on is left alone. Without swaync nothing happens.

### The library

The recordings list is a scan of `meeting.dir` (folders named by start time
that hold chunks) and `screen.dir` (`recording-*.mp4`), newest first, at most
`drawer.recent`. Lengths come from the WAV headers (or the file size, while
pw-record has not written the size yet) and the MP4 `moov/mvhd` box. A
meeting's transcript is `transcript.md`, or the old scripts'
`transcript.txt`. The daemon scans at start, after a recording or transcript
ends, and when the drawer opens.

## Transcripts

A stopped meeting is queued when `meeting.transcribe` is on. The queue lives
in `~/.local/state/logbook/queue.json` and survives a reboot. One transcript
runs at a time.

`transcribe.runner`:

- `auto`: Switchyard when its socket answers and it has the task
  `transcribe.task`, else direct.
- `switchyard`: `{"cmd":"run","id":"logbook-transcribe","params":{"dir":...}}`.
  The task is heavy, so it waits for the main line behind builds and runs
  with Switchyard's memory cap. `logbook setup` writes the task to
  `~/.config/switchyard/tasks.d/logbook.toml`.
- `direct`: `<transcribe.wrap> logbook transcribe <dir>` (`lowprio` by
  default, `nice -n 19` when the wrapper is missing) as an owned child, with
  its output in `~/.local/state/logbook/transcribe.log`.

`logbook transcribe <dir>` is the worker either way:

1. Lists the chunks and their lengths, reads `marks.json`.
2. Plans pieces: each chunk is cut at the marks inside it. A mark within 1.5
   seconds of a chunk's edge moves onto the edge. Marks after the last word
   still get their heading.
3. Runs `transcribe.command` on each piece (a cut piece is a temporary WAV
   with a clean header), reporting `progress` to the daemon.
4. Writes `transcript.md` (through a temporary file) and reports `finished`.

```markdown
# Meeting · Wed 11 Nov 2026 10:02

52 min · 4 chunks · 2 marks · voxtype

What was said, one paragraph per line the transcriber printed.

## Mark 1 · 00:31:12

What was said after the mark.
```

While Switchyard holds a transcript, the daemon asks its snapshot every 3
seconds for the task's phase and what it waits behind ("det33 build"). A task
that ends without the worker reporting (out of memory, killed) fails the
job with Switchyard's message. A direct run that exits without reporting
fails with its exit status.

## Who has the mic

A PipeWire thread follows the registry: nodes, links and clients. A user of
the mic is a `Stream/Input/Audio` node, without `stream.monitor = true` (peak
meters), linked from an `Audio/Source*` or `Audio/Duplex` node. A stream
linked from a sink's monitor (wf-recorder, a visualizer) is not on the mic.
Capture streams are bound for their info, which names the process behind a
pipewire-pulse stream (`application.process.binary`, `application.process.id`);
native clients fall back to the client's `pipewire.sec.pid`.

- A stream whose pid is one of Logbook's recorders is **ours**.
- A stream from voxtype is **dictation**.
- Everything else is another app, shown by binary (`chrome`, `zoom`).
  `mic.ignore` hides programs by binary or app name.

When PipeWire is not answering the lamp only knows Logbook's own
recordings; `logbook status` and the drawer say so. The thread reconnects
with backoff.

## Dictation

voxtype keeps `$XDG_RUNTIME_DIR/voxtype/state` (`idle`, `recording`,
`transcribing`) and `pid`. The daemon watches that folder with inotify (and
`$XDG_RUNTIME_DIR` until the folder exists) and voxtype's pid with a pidfd,
so a crash turns dictation off without polling. A dead pid reads as
`stopped`.

## The bar

`logbook bar` subscribes and prints one JSON line per change:

| Lamp | When | Text | Class |
|---|---|---|---|
| recording | a meeting or screen capture records | `bar.icon_rec` and `hh:mm:ss` | `recording`, then `meeting` and/or `screen` |
| dictating | voxtype listens | `bar.icon_dictate listening` | `dictating` |
| processing | the loudness pass, or voxtype typing | `bar.icon_busy saving` / `typing` | `processing` |
| transcribing | a transcript runs or waits | `bar.icon_busy 2/3` or `queued` | `transcribing` |
| mic | another app has the mic | `bar.icon_mic chrome` | `mic` |
| off | nothing listens | `bar.icon_off` | `off` |

The first row that holds wins. Any lamp but `mic` adds the class `shared`
while another app also has the mic. With the daemon gone the module shows
`offline` and starts the daemon again at most every 10 seconds.

## Anchoring and theme

As in Roundhouse, Switchyard and Tannoy: `logbook toggle` asks Hyprland's
IPC for the cursor, monitor and bar edge before it talks to the daemon, and
the drawer springs out of the clicked icon. The drawer takes its font and
colors from waybar's `style.css` (`theme.from = "waybar"`), then the user's
`~/.config/logbook/style.css`. Tokens: `lb_panel`, `lb_well`, `lb_border`,
`lb_fg`, `lb_dim`, `lb_tile`, `lb_tile_border`, `lb_tile_hover`, `lb_accent`,
`lb_rec` (the bar's `@red`), `lb_busy` (`@yellow`), `lb_ok` (`@green`),
`lb_idle`.

## Paths

| Path | What |
|---|---|
| `$XDG_RUNTIME_DIR/logbook/logbook.sock` | Daemon socket, mode 0600. |
| `$XDG_RUNTIME_DIR/logbook/daemon.lock` | Held by the running daemon. |
| `$XDG_RUNTIME_DIR/logbook/live.json` | What records now, with process groups. |
| `~/.config/logbook/config.toml`, `style.css` | Config and drawer style. |
| `~/.local/state/logbook/queue.json` | Transcripts waiting. |
| `~/.local/state/logbook/{daemon,board,transcribe}.log` | Logs. |

`LOGBOOK_RUNTIME_DIR`, `LOGBOOK_STATE_DIR`, `LOGBOOK_CONFIG_DIR` and
`LOGBOOK_VOXTYPE_DIR` move them, for development.

## Protocol

Newline-delimited JSON on the socket, one reply per request, except
`subscribe`, which streams.

```json
{"cmd":"start","what":"meeting"}
{"cmd":"start","what":"screen","geometry":"10,20 300x200"}
{"cmd":"stop"}
{"cmd":"stop","what":"screen"}
{"cmd":"mark"}
{"cmd":"queue","dir":"/home/n/Audio/meetings/2026-11-11_10-02-00"}
{"cmd":"forget","dir":"..."}
{"cmd":"rescan"}
{"cmd":"snapshot"}
{"cmd":"subscribe","kind":"bar"}
{"cmd":"subscribe","kind":"board"}
{"cmd":"toggle","anchor":{...}}
{"cmd":"show","anchor":{...}}
{"cmd":"hide"}
{"cmd":"board_visible","visible":true}
{"cmd":"progress","dir":"...","done":1,"total":3,"done_secs":900.0,"total_secs":2700.0}
{"cmd":"finished","dir":"...","ok":true}
```

Replies are `{"ok":true,"message":"..."}` or `{"ok":false,"error":"..."}`.
Board events: `{"event":"snapshot","snapshot":{...}}`, `show`, `toggle`,
`hide`.

## Config keys

```toml
[bar]
icon_off = "󰍬"        # mic closed
icon_mic = "󰍬"        # another app has the mic
icon_rec = "󰑊"        # Logbook records
icon_dictate = "󰍬"
icon_busy = "󰔟"       # saving or transcribing
show_app = true       # name the app holding the mic
namespace = "waybar"  # layer-shell namespace of the bar

[meeting]
dir = "~/Audio/meetings"
chunk = 900           # seconds per chunk
source = ""           # pw-record --target; empty is the default input
dnd = true
transcribe = true     # queue a transcript on stop

[screen]
dir = "~/Videos"
audio = "monitor"     # "monitor", "none", or a PipeWire source name
loudnorm = true
dnd = true

[transcribe]
runner = "auto"       # "auto", "switchyard" or "direct"
task = "logbook-transcribe"
command = ["voxtype", "transcribe"]   # the WAV path is appended
wrap = ["lowprio"]    # what a direct run is wrapped in

[mic]
watch = true
ignore = []           # binaries or app names

[drawer]
width = 440
max_height = 0.8
gap = 8
recent = 30

[theme]
from = "waybar"
# accent = "#88c0d0"
# font = "PragmataPro Liga"
```

## Keyboard (drawer)

| Key | Action |
|---|---|
| 1, 2, 3, Tab | Now, Recordings, Transcripts |
| r | start or stop a meeting |
| m | mark |
| s | start (via slurp) or stop a screen capture |
| Esc, a click outside | close |
