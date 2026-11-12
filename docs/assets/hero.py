#!/usr/bin/env python3
"""Writes docs/assets/hero.svg: the README's animated hero.

A 16 second loop on the same desktop as the siblings' heroes, under the
floating bar of pills. A click on Join in a video call hands Chrome the mic,
and the Logbook lamp turns frost and names it. A click on the lamp springs
the drawer out of it: the mic is in use, Chrome has it, and nothing is
recording. Record meeting puts the lamp on air: red with a running clock,
do not disturb on, the song title off the bar. Time skips ahead, chunk 2
starts, a Mark lands, and Stop hands the meeting to its transcript: the lamp
goes amber through 1/3, 2/3, 3/3, then back to frost while Chrome still has
the mic, and the recording appears with Copy. A click outside closes the
drawer, Leave ends the call, and the lamp goes dim.

Every name, time and file is made up.

Pure CSS animation inside the SVG, so GitHub plays it in an <img>. With
reduced motion it is a still of the open drawer on air.

The drawer is drawn by `drawer()` in its own coordinates from a `Now`
state, with nothing from the bar or the desktop, so other scenes can reuse
it.

    python3 docs/assets/hero.py
"""

from dataclasses import dataclass, field
from pathlib import Path

W, H = 1200, 660
T = 16  # loop seconds

# Nord, matching the waybar theme and the siblings' heroes.
BG0, BG1 = "#14171c", "#0d0f12"
PILL, PILL_LINE = "rgba(40,42,46,.84)", "rgba(76,86,106,.55)"
HOVER, WS_ON = "rgba(129,161,193,.18)", "rgba(129,161,193,.26)"
BAR_FG, BRIGHT, BAR_DIM = "#c5c8c6", "#d8dee9", "#616e88"
FROST, GREEN, YELLOW, RED, PURPLE, BLUE = "#88c0d0", "#a3be8c", "#ebcb8b", "#bf616a", "#b48ead", "#5e81ac"
DARK = "#1d2024"
FONT = "JetBrains Mono, DejaVu Sans Mono, Menlo, Consolas, monospace"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


# ================================================================ the drawer
#
# Logbook's drawer (crates/logbook-board: drawer.rs, style.css) on its Now
# tab, at its real size: 440 wide, 14 padding, the waybar theme's colors.
# `drawer(pen, state)` draws it with its top-left at (0, 0); `drawer_size`
# says how tall a state is (the panel grows upward from the bar). A pen is
# anything with `add(svg_string)` and `text(x, y, s, size, fill, weight,
# anchor, extra)`, such as dot-files' svgkit.Svg. Glyphs are drawn, not
# typed, so no icon font is needed. Uses no globals outside this section.

DRAWER_W = 440
LB = {  # the drawer's tokens with theme.from = "waybar" (theme.rs)
    "panel": "#282a2e", "well": "#1a1b1e", "border": "#41464e", "fg": "#c5c8c6",
    "dim": "#7e8182", "tile": "#303236", "tile_border": "#3b3d40",
    "accent": "#88c0d0", "rec": "#bf616a", "busy": "#ebcb8b", "idle": "#555b64",
}
# The lamp's states: (header dot and chip color, chip text). The bar's own
# text for each is in LAMP_TEXT further down.
LAMPS = {
    "off": ("idle", "off air"),
    "mic": ("accent", "mic in use"),
    "recording": ("rec", "on air"),
    "dictating": ("rec", "listening"),
    "processing": ("busy", "saving"),
    "transcribing": ("busy", "transcribing"),
}
METER_CSS = (
    ".lbm { transform-box: fill-box; transform-origin: center; animation: lbm .5s ease-in-out infinite alternate; }\n"
    "@keyframes lbm { from { transform: scaleY(.25); } }\n"
    "@media (prefers-reduced-motion: reduce) { .lbm { animation: none !important; } }"
)


@dataclass
class Meeting:
    clock: str = "00:41:12"
    chunk: int = 3
    source: str = "default input"
    marks: list = field(default_factory=list)  # clocks, oldest first
    dnd: bool = True  # Logbook turned do not disturb on


@dataclass
class Job:
    meeting: str = "today 14:02"
    runner: str = "switchyard"  # or lowprio, starting
    piece: int = 1
    total: int = 3
    eta: str = "~1 min left"
    fraction: float = 0.0
    behind: str = ""  # what Switchyard has it queued behind


@dataclass
class Row:
    kind: str  # meeting | screen
    when: str  # today 10:02
    sub: str  # 30 min · transcript ready
    action: str = ""  # Copy | Open | Transcribe | "" (folder only)


@dataclass
class Now:
    lamp: str = "off"
    meeting: Meeting | None = None
    screen: str = ""  # "" | recording | saving
    screen_clock: str = "00:00:00"
    dictation: str = ""  # "" | listening | typing
    voxtype: bool = True  # voxtype runs, so Dictate shows
    others: list = field(default_factory=list)  # (binary, app name, source)
    job: Job | None = None
    recent: list = field(default_factory=list)
    total: int = 0
    today: int = 0
    folder: str = "~/Audio"


P = 14  # panel padding
IW = DRAWER_W - 2 * P


def _cw(size):
    """A monospace character's advance."""
    return size * 0.6


def _fit(s, size, room):
    n = int(room / _cw(size))
    return s if len(s) <= n else s[: max(1, n - 1)].rstrip() + "…"


# ---- glyphs, each centered on (cx, cy) about 14px across

def g_mic(pen, cx, cy, c, s=1.0):
    pen.add(f'<g transform="translate({cx:.1f},{cy:.1f}) scale({s})" fill="none" stroke="{c}" stroke-width="1.7" stroke-linecap="round">'
            f'<rect x="-2.8" y="-7" width="5.6" height="9.5" rx="2.8" fill="{c}" stroke="none"/>'
            '<path d="M-5.2,-0.8 a5.2,5.2 0 0 0 10.4,0 M0,4.4 v3"/></g>')


def g_people(pen, cx, cy, c):
    pen.add(f'<g transform="translate({cx:.1f},{cy:.1f})" fill="{c}">'
            '<circle cx="0" cy="-3" r="2.7"/><path d="M-4.4,5 a4.4,4.2 0 0 1 8.8,0 Z"/>'
            '<circle cx="-5.6" cy="-1.6" r="2"/><path d="M-9,5 a3.4,3.4 0 0 1 4.2,-3.3 a5.6,5.6 0 0 0 -0.6,3.3 Z"/>'
            '<circle cx="5.6" cy="-1.6" r="2"/><path d="M9,5 a3.4,3.4 0 0 0 -4.2,-3.3 a5.6,5.6 0 0 1 0.6,3.3 Z"/></g>')


def g_screen(pen, cx, cy, c):
    pen.add(f'<g transform="translate({cx:.1f},{cy:.1f})" fill="none" stroke="{c}" stroke-width="1.6" stroke-linejoin="round">'
            '<rect x="-6.5" y="-5.5" width="13" height="9" rx="1.2"/><path d="M-3,6.5 h6 M0,3.5 v3"/></g>')


def g_copy(pen, cx, cy, c):
    pen.add(f'<g transform="translate({cx:.1f},{cy:.1f})" fill="none" stroke="{c}" stroke-width="1.4">'
            '<rect x="-2" y="-3.6" width="7" height="9" rx="1.3"/><path d="M-4.6,3 v-7.3 a1.3,1.3 0 0 1 1.3,-1.3 h5.6"/></g>')


def g_folder(pen, cx, cy, c):
    pen.add(f'<path d="M{cx - 6.5:.1f},{cy - 4.5:.1f} h4.4 l1.6,1.8 h6.4 a1,1 0 0 1 1,1 v1 h-10.4 l-3,6.4 Z '
            f'M{cx - 6.1:.1f},{cy + 5.2:.1f} l2.6,-6.2 h11.2 l-2.6,6.2 Z" fill="{c}"/>')


def g_stop(pen, cx, cy, c):
    pen.add(f'<rect x="{cx - 4:.1f}" y="{cy - 4:.1f}" width="8" height="8" rx="1" fill="{c}"/>')


def g_flag(pen, cx, cy, c):
    pen.add(f'<path d="M{cx - 4:.1f},{cy + 6:.1f} V{cy - 6:.1f} H{cx + 4.5:.1f} l-1.6,2.6 l1.6,2.6 H{cx - 4:.1f}" '
            f'fill="{c}" stroke="{c}" stroke-width="1.5" stroke-linejoin="round"/>')


def g_play(pen, cx, cy, c):
    pen.add(f'<path d="M{cx - 3:.1f},{cy - 4.5:.1f} V{cy + 4.5:.1f} L{cx + 4:.1f},{cy:.1f} Z" fill="{c}"/>')


def g_doc(pen, cx, cy, c):
    pen.add(f'<g transform="translate({cx:.1f},{cy:.1f})"><path d="M-4.5,-6.5 h5.5 l4,4 v9 h-9.5 Z" fill="{c}"/>'
            f'<path d="M-2.4,0.5 h5 M-2.4,3.4 h5" stroke="{LB["panel"]}" stroke-width="1.2"/></g>')


GLYPHS = {"mic": g_mic, "people": g_people, "screen": g_screen, "copy": g_copy, "folder": g_folder,
          "stop": g_stop, "flag": g_flag, "play": g_play, "doc": g_doc}


def _label_row(pen, x, y, w, left, right="", left_fill=None):
    """lab_row(): the small bold caps, one left and one right."""
    ls = ' letter-spacing="1"'
    if right:
        pen.text(x + w, y, right.upper(), 10, LB["dim"], 700, "end", ls)
        w -= len(right) * 7 + 8
    pen.text(x, y, _fit(left.upper(), 11.7, w), 10, left_fill or LB["dim"], 700, "start", ls)


def _button(pen, x, y, label, glyph=None, kind="", small=False, spots=None, name=None):
    """A button left-aligned at x; returns its width. kind: primary, danger."""
    size = 11.5 if small else 12.5
    h = 26 if small else 30
    pad = 9 if small else 10
    if not label:  # an icon button
        pad = 7
    w = 2 * pad + 2 + (14 if glyph else 0) + (4 if glyph and label else 0) + len(label) * _cw(size)
    fill, line, fg = LB["tile"], LB["tile_border"], LB["fg"]
    if kind == "primary":
        fill, line, fg = LB["accent"], LB["accent"], LB["well"]
    elif kind == "danger":
        fill, line, fg = LB["rec"], LB["rec"], LB["well"]
    pen.add(f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h}" rx="9" fill="{fill}" stroke="{line}"/>')
    gx = x + pad + 1
    if glyph:
        GLYPHS[glyph](pen, gx + 7, y + h / 2, fg)
        gx += 18
    if label:
        pen.text(gx, y + h / 2 + size * 0.36, label, size, fg, 700 if kind else 400)
    if spots is not None and name:
        spots[name] = (x + w / 2, y + h / 2)
    return w


def _right_buttons(pen, right, y, specs, spots):
    """Buttons packed right-to-left from `right`; returns their left edge."""
    x = right
    for label, glyph, name in reversed(specs):
        size = 11.5
        pad = 9 if label else 7
        w = 2 * pad + 2 + 14 + (4 if label else 0) + len(label) * _cw(size)
        x -= w
        _button(pen, x, y, label, glyph, small=True, spots=spots, name=name)
        x -= 8
    return x + 8


def _card(pen, y, h, kind=""):
    tints = {"rec": (LB["rec"], ".07", ".35"), "busy": (LB["busy"], ".06", ".3")}
    if kind in tints:
        c, fo, so = tints[kind]
        pen.add(f'<rect x="{P}" y="{y:.1f}" width="{IW}" height="{h:.1f}" rx="10" fill="{c}" fill-opacity="{fo}" stroke="{c}" stroke-opacity="{so}"/>')
    else:
        pen.add(f'<rect x="{P}" y="{y:.1f}" width="{IW}" height="{h:.1f}" rx="10" fill="{LB["tile"]}" stroke="{LB["tile_border"]}"/>')


def _meter(pen, right, cy):
    """The level meter: 18 bars, 6px apart, 26 tall, in the record color."""
    heights = [.3, .55, .8, .45, .95, .6, .35, .7, 1, .5, .4, .85, .65, .3, .75, .9, .5, .6]
    x0 = right - 18 * 6
    for i, v in enumerate(heights):
        bh = max(2.0, 26 * (0.12 + 0.88 * v))
        d = 0.31 + (i * 7 % 5) * 0.07
        pen.add(f'<rect class="lbm" style="animation-duration:{d:.2f}s;animation-delay:-{i * 0.13:.2f}s" x="{x0 + i * 6 + 1.2:.1f}" '
                f'y="{cy - bh / 2:.1f}" width="3.6" height="{bh:.1f}" rx="1.8" fill="{LB["rec"]}"/>')


def _page(pen, s, y, clock, spots):
    """The Now page from y; returns the y below it."""
    ip = P + 10  # inside a card
    right = DRAWER_W - P - 10
    gap = 10
    # ---- what records now, or the mic card
    live = False
    if s.meeting:
        m = s.meeting
        h = 9 + 13 + 7 + 32 + 7 + (17 + 7 if m.marks else 0) + 30 + 9
        _card(pen, y, h, "rec")
        cy = y + 9
        _label_row(pen, ip, cy + 10, right - ip, f"Meeting · chunk {m.chunk} · {m.source}", "dnd on" if m.dnd else "")
        cy += 13 + 7
        if clock:
            clock(pen, ip, cy + 26)
        else:
            pen.text(ip, cy + 26, m.clock, 26, LB["rec"], 700)
        _meter(pen, right, cy + 16)
        cy += 32 + 7
        if m.marks:
            n = len(m.marks)
            pen.text(ip, cy + 13, f"{n} mark{'' if n == 1 else 's'} · last at {m.marks[-1]}", 13, LB["dim"])
            cy += 17 + 7
        x = ip + _button(pen, ip, cy, "Stop", "stop", "danger", spots=spots, name="stop") + 8
        _button(pen, x, cy, "Mark", "flag", spots=spots, name="mark")
        y += h + gap
        live = True
    if s.screen:
        saving = s.screen == "saving"
        h = 9 + 13 + 7 + (17 if saving else 32 + 7 + 30) + 9
        _card(pen, y, h, "busy" if saving else "rec")
        if saving:
            _label_row(pen, ip, y + 19, right - ip, "Screen · saving", "loudness pass")
            pen.add(f'<circle cx="{ip + 7}" cy="{y + 42}" r="5.5" fill="none" stroke="{LB["fg"]}" stroke-width="1.6" stroke-dasharray="20 15"/>')
            pen.text(ip + 20, y + 46.5, "recording-20261112-141530.mp4", 13, LB["fg"])
        else:
            _label_row(pen, ip, y + 19, right - ip, "Screen · 1280,40 1920x1080", "dnd on")
            pen.text(ip, y + 9 + 20 + 26, s.screen_clock, 26, LB["rec"], 700)
            _button(pen, ip, y + 9 + 20 + 39, "Stop", "stop", "danger", spots=spots, name="stop-screen")
        y += h + gap
        live = True
    if s.dictation:
        typing = s.dictation == "typing"
        h = 9 + 13 + 7 + 17 + (0 if typing else 7 + 30) + 9
        _card(pen, y, h, "rec")
        _label_row(pen, ip, y + 19, right - ip, "Dictation · voxtype", s.dictation)
        pen.text(ip, y + 29 + 13, "Typing out what you said…" if typing else "Release the key, or press Stop, to type it.", 13, LB["dim"])
        if not typing:
            _button(pen, ip, y + 29 + 24, "Stop", "stop", "danger", spots=spots, name="stop-dictation")
        y += h + gap
        live = True
    if not live:
        h = 9 + 13 + 7 + 17 + 7 + 30 + 9
        _card(pen, y, h)
        _label_row(pen, ip, y + 19, right - ip, "Mic", "in use" if s.others else "closed")
        line = "Logbook is not recording." if s.others else "Nothing is listening. No app has the mic open."
        pen.text(ip, y + 29 + 13, line, 13, LB["fg"])
        x = ip
        by = y + 29 + 24
        x += _button(pen, x, by, "Record meeting", "people", "primary", spots=spots, name="record-meeting") + 8
        x += _button(pen, x, by, "Record screen", "screen", spots=spots, name="record-screen") + 8
        if s.voxtype:
            _button(pen, x, by, "Dictate", "mic", spots=spots, name="dictate")
        y += h + gap
    # ---- other apps on the mic
    if s.others:
        h = 9 + 13 + 7 + len(s.others) * 18 + (len(s.others) - 1) * 7 + 9
        _card(pen, y, h)
        _label_row(pen, ip, y + 19, right - ip, "Also using the mic", " · ".join(o[0] for o in s.others))
        for i, (name, app, source) in enumerate(s.others):
            ry = y + 29 + i * 25
            g_mic(pen, ip + 7, ry + 9, LB["accent"], 0.85)
            who = name if not app or app.lower() == name.lower() else f"{name} ({app})"
            pen.text(right, ry + 13, source, 13, LB["dim"], anchor="end")
            pen.text(ip + 22, ry + 13, _fit(who, 13, right - ip - 30 - len(source) * 7.8), 13, LB["fg"])
        y += h + gap
    # ---- the transcript being made
    if s.job:
        j = s.job
        h = 9 + 13 + 7 + 17 + 7 + 5 + (7 + 13 if j.behind else 0) + 9
        _card(pen, y, h, "busy")
        _label_row(pen, ip, y + 19, right - ip, f"Transcribing · meeting {j.meeting}", j.runner)
        pen.text(ip, y + 29 + 13, f"Piece {j.piece} of {j.total}" if j.total else "Starting", 13, LB["fg"])
        pen.text(right, y + 29 + 13, j.eta, 13, LB["busy"], anchor="end")
        by = y + 29 + 24
        pen.add(f'<rect x="{ip}" y="{by}" width="{right - ip}" height="5" rx="2.5" fill="{LB["tile_border"]}"/>')
        pen.add(f'<rect x="{ip}" y="{by}" width="{max(5, (right - ip) * j.fraction):.1f}" height="5" rx="2.5" fill="{LB["busy"]}"/>')
        if j.behind:
            _label_row(pen, ip, by + 5 + 7 + 10, right - ip, "Queued behind", j.behind)
        y += h + gap
    # ---- the three most recent recordings
    if s.recent:
        _label_row(pen, P, y + 10, IW, "Recent", f"{s.total} in all")
        y += 13
        for i, r in enumerate(s.recent[:3]):
            y += 6
            rh = 45
            gx = P + 9 + 8
            (g_people if r.kind == "meeting" else g_screen)(pen, gx, y + rh / 2, LB["accent"])
            specs = []
            if r.action == "Copy":
                specs.append(("Copy", "copy", f"copy:{i}"))
            elif r.action == "Open":
                specs.append(("Open", "play", f"open:{i}"))
            elif r.action == "Transcribe":
                specs.append(("Transcribe", "doc", f"transcribe:{i}"))
            specs.append(("", "folder", f"folder:{i}"))
            left = _right_buttons(pen, DRAWER_W - P - 9, y + (rh - 26) / 2, specs, spots)
            tx = gx + 8 + 10
            room = left - 10 - tx
            title = f"{'Meeting' if r.kind == 'meeting' else 'Screen'} · {r.when}"
            pen.text(tx, y + 20, _fit(title, 13, room), 13, LB["fg"], 700)
            pen.text(tx, y + 36, _fit(r.sub, 11, room), 11, LB["dim"])
            y += rh
        y += 12 - gap
    return y


def _draw(pen, s, clock=None):
    """Lays out and draws the panel; returns (height, spots)."""
    spots = {}
    # ---- header: the lamp, LOGBOOK and what is under it, the chip
    color, chip = LAMPS[s.lamp]
    pen.add(f'<circle cx="{P + 5}" cy="30" r="4.5" fill="{LB[color]}"/>')
    pen.text(P + 20, 28, "LOGBOOK", 12, LB["fg"], 700, "start", ' letter-spacing="2"')
    pen.text(P + 20, 44, f"{s.today} recording{'' if s.today == 1 else 's'} today · {s.folder}", 11, LB["dim"])
    cw = len(chip) * _cw(11) + 18
    if s.lamp == "off":
        pen.add(f'<rect x="{DRAWER_W - P - cw:.1f}" y="20" width="{cw:.1f}" height="20" rx="10" fill="{LB["idle"]}" fill-opacity=".2"/>')
        pen.text(DRAWER_W - P - cw / 2, 34, chip, 11, LB["dim"], 700, "middle")
    else:
        pen.add(f'<rect x="{DRAWER_W - P - cw:.1f}" y="20" width="{cw:.1f}" height="20" rx="10" fill="{LB[color]}" fill-opacity=".15"/>')
        pen.text(DRAWER_W - P - cw / 2, 34, chip, 11, LB[color], 700, "middle")
    # ---- tabs: Now is checked (an accent line along its foot)
    x, y = P, 58
    for i, name in enumerate(["Now", "Recordings", "Transcripts"]):
        w = len(name) * _cw(12) + 24
        pen.add(f'<rect x="{x:.1f}" y="{y}" width="{w:.1f}" height="28" rx="9" fill="{LB["tile"]}" stroke="{LB["tile_border"]}"/>')
        if i == 0:
            pen.add(f'<path d="M{x + 1:.1f},{y + 19} v5 a3,3 0 0 0 3,3 h{w - 8:.1f} a3,3 0 0 0 3,-3 v-5" fill="none" stroke="{LB["accent"]}" stroke-width="2"/>')
        pen.text(x + w / 2, y + 18.5, name, 12, LB["fg"], 400, "middle")
        spots[f"tab:{name.lower()}"] = (x + w / 2, y + 14)
        x += w + 4
    y = _page(pen, s, 98, clock, spots)
    # ---- the keys
    y += 12
    pen.text(P, y + 11, "1-3 tabs · r record meeting · m mark · s screen · Esc close", 10.5, LB["dim"])
    return y + 14 + P, spots


class _Sub:
    """Records a drawing, so the panel can go under it once its height is known."""

    def __init__(self):
        self.out = []

    def add(self, s):
        self.out.append(("add", s))

    def text(self, *a, **k):
        self.out.append(("text", a, k))


def drawer_size(s):
    """(width, height) of the drawer in state `s`."""
    return DRAWER_W, _draw(_Sub(), s)[0]


def drawer(pen, s, clock=None):
    """Draws the drawer in state `s` at (0, 0) and returns its clickable
    spots, name -> (x, y) in drawer coordinates: tab:now, record-meeting,
    record-screen, dictate, stop, mark, copy:0, folder:0 and so on.
    `clock(pen, x, y)`, when given, draws the meeting's big clock (26px
    bold, rec color) at its baseline, so a scene can make it tick. The
    panel's drop shadow is `url(#shadow)`."""
    sub = _Sub()
    h, spots = _draw(sub, s, clock)
    pen.add(f'<rect x=".5" y=".5" width="{DRAWER_W - 1}" height="{h - 1}" rx="16" fill="{LB["panel"]}" '
            f'fill-opacity=".97" stroke="{LB["border"]}" filter="url(#shadow)"/>')
    for op in sub.out:
        if op[0] == "add":
            pen.add(op[1])
        else:
            pen.text(*op[1], **op[2])
    return spots


# ================================================================ the scene

out, kfs = [], []


def add(s):
    out.append(s)


class Pen:
    """The drawer's pen for this file."""

    def add(self, s):
        add(s)

    def text(self, x, y, s, size=13, fill=BAR_FG, weight=400, anchor="start", extra=""):
        w = f' font-weight="{weight}"' if weight != 400 else ""
        a = f' text-anchor="{anchor}"' if anchor != "start" else ""
        add(f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}"{w}{a}{extra}>{esc(s)}</text>')


pen = Pen()
text = pen.text


def pct(sec):
    return f"{max(0.0, min(100.0, sec / T * 100)):.2f}%"


def keys(pts):
    """A keyframe set from (seconds, declarations) points; returns its name."""
    name = f"k{len(kfs)}"
    rows = " ".join(f"{pct(t)} {{ {v}; }}" for t, v in pts)
    kfs.append(f"@keyframes {name} {{ {rows} }}")
    return name


def anim(prop, pts):
    return keys([(t, f"{prop}: {v}") for t, v in pts])


def fade(spans, ramp=0.04):
    """Opacity keyframes visible during each (start, end) span."""
    merged = []
    for s, e in sorted(spans):  # spans that touch are one
        if merged and s <= merged[-1][1] + 1e-6:
            merged[-1][1] = max(merged[-1][1], e)
        else:
            merged.append([s, e])
    pts = []
    for s, e in merged:
        if s <= 0:
            pts.append((0, 1))
        else:
            pts += [(0, 0)] if not pts else []
            pts += [(s, 0), (s + ramp, 1)]
        if e >= T:
            pts.append((T, 1))
        else:
            pts += [(e, 1), (e + ramp, 0)]
    if pts[-1][0] < T:
        pts.append((T, 0))
    return anim("opacity", pts)


def style(*names, extra=""):
    return f'class="a" style="animation-name:{",".join(names)};{extra}"'


# ---------------------------------------------------------------- the story, in seconds

CLICK_JOIN = 0.8
MIC_ON = 1.0  # Chrome opens the mic
CLICK_LAMP, OPEN = 2.0, 2.08
CLICK_RECORD = 3.3
REC = 3.4  # on air
LAPSE1 = 4.9  # time skips ahead to 00:21:04
CLICK_MARK = 6.7
MARKED = 6.8
LAPSE2 = 7.8  # and again to 00:24:12
CLICK_STOP = 9.2
STOPPED = 9.3  # the transcript is queued, piece 1 of 3
PIECES = (STOPPED, 10.0, 10.7)
READY = 11.4
CLICK_COPY = 12.0
CLICK_OUT, CLOSED = 12.8, 13.25
CLICK_LEAVE = 13.6
MIC_OFF = 13.8  # Chrome lets go of the mic
RESET = 15.0
STILL = 8.4  # what reduced motion shows: on air, one mark, the drawer open


def at(spans, t=STILL):
    return any(a <= t < b for a, b in spans)


def group(spans, ramp=0.04, extra=""):
    """A group shown during `spans`; the still decides its resting opacity."""
    add(f'<g {style(fade(spans, ramp), extra=extra)} opacity="{1 if at(spans) else 0}">')


# The meeting's clock: (from, text). The skips run like a fast-forward.
CLOCKS = [
    (REC, "00:00:00"), (REC + 1, "00:00:01"),
    (LAPSE1, "00:06:40"), (LAPSE1 + 0.15, "00:13:20"), (LAPSE1 + 0.3, "00:19:10"), (LAPSE1 + 0.45, "00:21:04"),
    (LAPSE1 + 1.45, "00:21:05"), (LAPSE1 + 2.45, "00:21:06"),
    (LAPSE2, "00:22:30"), (LAPSE2 + 0.15, "00:24:12"), (LAPSE2 + 1.15, "00:24:13"),
]
MARK_AT = "00:21:05"
CHUNK2 = LAPSE1 + 0.3  # past 15:00, the second chunk starts
# Timetable's clock moves with it.
TIMES = [(0, "14:02"), (LAPSE1, "14:08"), (LAPSE1 + 0.15, "14:15"), (LAPSE1 + 0.3, "14:21"),
         (LAPSE1 + 0.45, "14:23"), (LAPSE2, "14:24"), (LAPSE2 + 0.15, "14:26"), (RESET, "14:02")]


def steps(frames, end):
    """(start, end, *rest) for each (start, *rest) frame, up to `end`."""
    return [(f[0], frames[i + 1][0] if i + 1 < len(frames) else end, *f[1:]) for i, f in enumerate(frames)]


# ---------------------------------------------------------------- the drawer's states

OLDER = [
    Row("meeting", "today 10:02", "30 min · transcript ready", "Copy"),
    Row("screen", "today 09:15", "2:14 · recording-20261112-091532.mp4", "Open"),
    Row("meeting", "yesterday 14:30", "31 min · 2 marks · transcript ready", "Copy"),
]
CHROME = [("chrome", "Google Chrome", "Shure MV7")]
NEW = "24 min · 1 mark"


def job(piece, fraction, eta):
    return Job("today 14:02", "switchyard", piece, 3, eta, fraction)


STATES = [  # (from, to, state)
    (0, REC, Now("mic", others=CHROME, recent=OLDER, total=18, today=2)),
    (REC, CHUNK2, Now("recording", Meeting(chunk=1), others=CHROME, recent=OLDER, total=18, today=2)),
    (CHUNK2, MARKED, Now("recording", Meeting(chunk=2), others=CHROME, recent=OLDER, total=18, today=2)),
    (MARKED, STOPPED, Now("recording", Meeting(chunk=2, marks=[MARK_AT]), others=CHROME, recent=OLDER, total=18, today=2)),
    (PIECES[0], PIECES[1], Now("transcribing", others=CHROME, job=job(1, 0.04, "~1 min left"),
                               recent=[Row("meeting", "today 14:02", f"{NEW} · transcribing")] + OLDER, total=19, today=3)),
    (PIECES[1], PIECES[2], Now("transcribing", others=CHROME, job=job(2, 0.62, "~1 min left"),
                               recent=[Row("meeting", "today 14:02", f"{NEW} · transcribing")] + OLDER, total=19, today=3)),
    (PIECES[2], READY, Now("transcribing", others=CHROME, job=job(3, 0.87, "under a minute left"),
                           recent=[Row("meeting", "today 14:02", f"{NEW} · transcribing")] + OLDER, total=19, today=3)),
    (READY, T, Now("mic", others=CHROME, recent=[Row("meeting", "today 14:02", f"{NEW} · transcript ready", "Copy")] + OLDER,
                   total=19, today=3)),
]

# ---------------------------------------------------------------- the bar's layout

BAR_Y, BAR_H = 620, 34  # 34 tall, floating 6px off the bottom
MID = BAR_Y + 22  # text baseline
SIZE, CH = 14, 8.4  # bar font and its advance
CY = BAR_Y + BAR_H / 2

# The lamp: what `logbook bar` prints in each state, and its class color
# from style.css. The first character stands for the glyph.
LAMP_TEXT = {
    "off": ("mic", "", "off", BAR_DIM),
    "mic": ("mic", "chrome", "mic", FROST),
    "recording": ("rec", "00:41:12", "recording meeting shared", RED),
    "dictating": ("mic", "listening", "dictating", RED),
    "processing": ("busy", "saving", "processing", YELLOW),
    "transcribing": ("busy", "1/3", "transcribing", YELLOW),
}
LAMPS_FRAMES = [(0, "off", "")]  # (from, state, words)
LAMPS_FRAMES.append((MIC_ON, "mic", "chrome"))
LAMPS_FRAMES += [(t, "recording", c) for t, c in CLOCKS]
LAMPS_FRAMES += [(PIECES[0], "transcribing", "1/3"), (PIECES[1], "transcribing", "2/3"), (PIECES[2], "transcribing", "3/3")]
LAMPS_FRAMES += [(READY, "mic", "chrome"), (MIC_OFF, "off", "")]
DND = [(REC + 0.05, STOPPED + 0.05)]  # Logbook holds do not disturb while on air


def lamp_w(words):
    return 16 + CH + (CH * (1 + len(words)) if words else 0)


R = W - 6
TRAY_W = 86
TRAY_X = R - TRAY_W
CLOCK_W = 24 + 17 * CH
CLOCK_X = TRAY_X - 6 - CLOCK_W
DESK_R = CLOCK_X - 6


def desk_w(words):
    return 4 + lamp_w(words) + 16 + CH + 4


SONG = "Marlow Fen · Tides"

# ---------------------------------------------------------------- scene

css = f"""
text {{ white-space: pre; }}
.a {{ animation-duration: {T}s; animation-iteration-count: infinite; animation-timing-function: linear; animation-fill-mode: both; }}
{METER_CSS}
@media (prefers-reduced-motion: reduce) {{
  .a {{ animation: none !important; }}
  #cursor, .ripple {{ display: none; }}
}}
"""

ALT = (
    "A floating waybar, its desk pill holding Logbook's lamp: a dim mic. A click on Join in a video call in Chrome "
    "turns the lamp frost with the word chrome. A click on the lamp springs the Logbook drawer out of it on the Now "
    "tab: the mic is in use and Logbook is not recording, with Record meeting, Record screen and Dictate, then "
    "chrome on the Shure MV7, then the three most recent recordings. Record meeting puts it on air: the lamp turns "
    "red with a running clock, the do not disturb bell turns amber and the song title leaves the bar, and the "
    "drawer shows the meeting with a big red clock, a level meter, Stop and Mark. Time skips ahead to chunk 2, a "
    "click on Mark adds 1 mark at 00:21:05, and a click on Stop at 00:24:13 hands the meeting to its transcript: "
    "the lamp turns amber and counts 1/3, 2/3, 3/3 while the drawer shows the transcript running in Switchyard "
    "with a progress bar, do not disturb turns off and the song comes back. When the transcript is ready the lamp "
    "is frost again, since Chrome still has the mic, and the meeting from 14:02 tops the recent list with Copy. Copy "
    "is clicked, a click outside closes the drawer, and Leave ends the call: the lamp goes dim."
)

add(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="{FONT}" xml:space="preserve" role="img" aria-label="{esc(ALT)}">')
STYLE_AT = len(out)
add("")  # the <style> goes here once every keyframe exists
add(f"""<defs>
<linearGradient id="wall" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{BG0}"/><stop offset="1" stop-color="{BG1}"/></linearGradient>
<radialGradient id="haze1" cx=".85" cy="0" r=".75"><stop offset="0" stop-color="{FROST}" stop-opacity=".13"/><stop offset="1" stop-color="{FROST}" stop-opacity="0"/></radialGradient>
<radialGradient id="haze2" cx=".05" cy="1" r=".6"><stop offset="0" stop-color="{PURPLE}" stop-opacity=".09"/><stop offset="1" stop-color="{PURPLE}" stop-opacity="0"/></radialGradient>
<linearGradient id="dusk" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3b2b4a"/><stop offset=".7" stop-color="#b8706a"/><stop offset="1" stop-color="#d08770"/></linearGradient>
<filter id="shadow" x="-20%" y="-20%" width="140%" height="150%"><feDropShadow dx="0" dy="18" stdDeviation="22" flood-color="#000" flood-opacity=".65"/></filter>
<clipPath id="screen"><rect width="{W}" height="{H}" rx="18"/></clipPath>
</defs>""")
add('<g clip-path="url(#screen)">')
add(f'<rect width="{W}" height="{H}" fill="url(#wall)"/><rect width="{W}" height="{H}" fill="url(#haze1)"/><rect width="{W}" height="{H}" fill="url(#haze2)"/>')

# ---------------------------------------------------------------- notes, beside the call

NX, NY, NW, NH = 644, 28, 528, 572
add(f'<rect x="{NX}" y="{NY}" width="{NW}" height="{NH}" rx="12" fill="#1a1d22" stroke="#2a2e35"/>')
for i, (indent, lw, col) in enumerate([(0, 180, "#5e81ac99"), (0, 0, ""), (0, 300, "#434c5e"), (0, 340, "#434c5e"), (0, 260, "#434c5e"),
                                       (0, 0, ""), (0, 140, "#88c0d055"), (24, 280, "#3b4252"), (24, 220, "#3b4252"), (24, 310, "#3b4252"),
                                       (0, 0, ""), (0, 120, "#88c0d055"), (24, 250, "#3b4252"), (24, 190, "#3b4252")]):
    if lw:
        add(f'<rect x="{NX + 24 + indent}" y="{NY + 30 + i * 26}" width="{lw}" height="9" rx="4" fill="{col}"/>')

# ---------------------------------------------------------------- the call, in Chrome

CX, CY0, CW, CHT = 28, 28, 600, 572  # the Chrome window
add(f'<rect x="{CX}" y="{CY0}" width="{CW}" height="{CHT}" rx="12" fill="#1a1d22" stroke="#2a2e35"/>')
add(f'<path d="M{CX},{CY0 + 40} v-28 a12,12 0 0 1 12,-12 h{CW - 24} a12,12 0 0 1 12,12 v28 Z" fill="#1f2328"/>')
for k, c in enumerate([RED, YELLOW, GREEN]):
    add(f'<circle cx="{CX + 18 + k * 16}" cy="{CY0 + 20}" r="5" fill="{c}" fill-opacity=".55"/>')
add(f'<rect x="{CX + 72}" y="{CY0 + 8}" width="190" height="26" rx="8" fill="#262a30"/>')
add(f'<rect x="{CX + 84}" y="{CY0 + 16}" width="10" height="10" rx="3" fill="{GREEN}" fill-opacity=".8"/>')
text(CX + 102, CY0 + 25.5, "Retro", 11.5, BAR_FG)
add(f'<rect x="{CX + 12}" y="{CY0 + 46}" width="{CW - 24}" height="24" rx="12" fill="#15171a"/>')
add(f'<rect x="{CX + 30}" y="{CY0 + 55}" width="200" height="6" rx="3" fill="#3b4252"/>')
VX, VY, VW, VH = CX + 14, CY0 + 82, CW - 28, CHT - 96  # the page

PEOPLE = [("AV", "Avery", "#5e81ac"), ("JO", "Jordan", PURPLE), ("SM", "Sam", GREEN), ("YO", "You", "#d08770")]


def avatar(cx, cy, r, ini, col):
    add(f'<circle cx="{cx:.1f}" cy="{cy:.1f}" r="{r}" fill="{col}" fill-opacity=".85"/>')
    text(cx, cy + r * 0.24, ini, r * 0.62, DARK, 700, "middle")


JOINED = [(CLICK_JOIN + 0.1, CLICK_LEAVE + 0.1)]
LEFT = [(CLICK_LEAVE + 0.1, RESET)]
LOBBY = [(0, CLICK_JOIN + 0.1), (RESET, T)]

# Before: the preview and Join now.
group(LOBBY, 0.2)
add(f'<rect x="{VX + 10}" y="{VY + 90}" width="330" height="220" rx="12" fill="#15171a"/>')
avatar(VX + 175, VY + 192, 42, "YO", "#d08770")
text(VX + 24, VY + 296, "You", 12, BAR_FG)
text(VX + 460, VY + 160, "Ready to join?", 20, BRIGHT, 700, "middle")
text(VX + 460, VY + 186, "Avery and Sam are here", 12, BAR_DIM, anchor="middle")
JOIN_C = (VX + 460, VY + 228)
add(f'<rect x="{JOIN_C[0] - 58}" y="{JOIN_C[1] - 18}" width="116" height="36" rx="18" fill="{FROST}"/>')
text(JOIN_C[0], JOIN_C[1] + 4.5, "Join now", 13, DARK, 700, "middle")
press = anim("opacity", [(0, 0), (CLICK_JOIN - 0.01, 0), (CLICK_JOIN, 1), (CLICK_JOIN + 0.25, 0), (T, 0)])
add(f'<rect x="{JOIN_C[0] - 58}" y="{JOIN_C[1] - 18}" width="116" height="36" rx="18" fill="#fff" fill-opacity=".2" {style(press)} opacity="0"/>')
add("</g>")

# During: four tiles, Avery talking, the controls.
group(JOINED, 0.2)
tw, th = (VW - 12) / 2, (VH - 70 - 12) / 2
for k, (ini, name, col) in enumerate(PEOPLE):
    x, y = VX + (k % 2) * (tw + 12), VY + (k // 2) * (th + 12)
    add(f'<rect x="{x:.1f}" y="{y:.1f}" width="{tw:.1f}" height="{th:.1f}" rx="10" fill="#15171a" stroke="{FROST if k == 0 else "#2a2e35"}" stroke-width="{2 if k == 0 else 1}"/>')
    avatar(x + tw / 2, y + th / 2 - 6, 34, ini, col)
    text(x + 12, y + th - 12, name, 12, BAR_FG)
by = VY + VH - 34
for k, label in enumerate(["mic", "cam", "share"]):
    cx = VX + VW / 2 - 110 + k * 56
    add(f'<circle cx="{cx}" cy="{by}" r="20" fill="#2e3440"/>')
    if label == "mic":
        g_mic(pen, cx, by, BRIGHT)
    elif label == "cam":
        add(f'<rect x="{cx - 8}" y="{by - 5.5}" width="11" height="11" rx="2" fill="{BRIGHT}"/><path d="M{cx + 3.5},{by} l5,-4 v8 z" fill="{BRIGHT}"/>')
    else:
        g_screen(pen, cx, by, BRIGHT)
LEAVE_C = (VX + VW / 2 + 70, by)
add(f'<rect x="{LEAVE_C[0] - 30}" y="{by - 20}" width="60" height="40" rx="20" fill="{RED}"/>')
add(f'<path d="M{LEAVE_C[0] - 11},{by + 2} q11,-9 22,0 l-1,4 l-5,-1.5 v-3 q-5,-2 -10,0 v3 l-5,1.5 z" fill="{BRIGHT}"/>')
press = anim("opacity", [(0, 0), (CLICK_LEAVE - 0.01, 0), (CLICK_LEAVE, 1), (CLICK_LEAVE + 0.25, 0), (T, 0)])
add(f'<rect x="{LEAVE_C[0] - 30}" y="{by - 20}" width="60" height="40" rx="20" fill="#fff" fill-opacity=".2" {style(press)} opacity="0"/>')
add("</g>")

# After: left.
group(LEFT, 0.2)
text(VX + VW / 2, VY + 200, "You left the meeting", 20, BRIGHT, 700, "middle")
add(f'<rect x="{VX + VW / 2 - 50}" y="{VY + 226}" width="100" height="34" rx="17" fill="none" stroke="{PILL_LINE}"/>')
text(VX + VW / 2, VY + 247.5, "Rejoin", 13, FROST, 700, "middle")
text(VX + VW / 2, VY + 290, "Return to home screen", 12, BAR_DIM, anchor="middle")
add("</g>")

# ---------------------------------------------------------------- the bar


def pill(x, w):
    add(f'<rect x="{x:.1f}" y="{BAR_Y + 0.5}" width="{w:.1f}" height="{BAR_H - 1}" rx="10" fill="{PILL}" stroke="{PILL_LINE}"/>')


# Workspace and title glyphs, drawn small.
def w_chrome(cx, cy, c):
    add(f'<circle cx="{cx}" cy="{cy}" r="6" fill="none" stroke="{c}" stroke-width="1.6"/><circle cx="{cx}" cy="{cy}" r="2.4" fill="{c}"/>'
        f'<path d="M{cx},{cy - 2.4} h5.6 M{cx - 2},{cy + 1.3} l-3,5 M{cx + 2},{cy + 1.3} l-2.6,-6.3" stroke="{c}" stroke-width="1.3"/>')


def w_slack(cx, cy, c):
    """A hash of four rounded bars."""
    for d in (-2.4, 2.4):
        add(f'<rect x="{cx + d - 1.3:.1f}" y="{cy - 6.5:.1f}" width="2.6" height="13" rx="1.3" fill="{c}"/>')
        add(f'<rect x="{cx - 6.5:.1f}" y="{cy + d - 1.3:.1f}" width="13" height="2.6" rx="1.3" fill="{c}"/>')


def w_vim(cx, cy, c):
    """Neovim's mark: two posts and the diagonal between them."""
    add(f'<path d="M{cx - 6:.1f},{cy - 4:.1f} l3,-3 v14 l-3,-3 Z M{cx + 6:.1f},{cy + 4:.1f} l-3,3 v-14 l3,3 Z '
        f'M{cx - 3:.1f},{cy - 7:.1f} l7,10.5 v3.5 l-1,0 l-6,-9 Z" fill="{c}"/>')


def w_term(cx, cy, c):
    add(f'<rect x="{cx - 6.5}" y="{cy - 5.5}" width="13" height="11" rx="2" fill="{c}"/>'
        f'<path d="M{cx - 4},{cy - 2} l2.4,2 l-2.4,2 M{cx},{cy + 2.5} h3" fill="none" stroke="#25272b" stroke-width="1.3"/>')


x = 6
spans = [("1", [w_chrome], True), ("2", [w_slack], False), ("3", [w_vim, w_term], False)]
ws_w = 4 + sum(16 + CH * (1 + 2 * len(g)) + 2 for _, g, _ in spans) + 4
pill(x, ws_w)
bx = x + 4
for n, glyphs, active in spans:
    bw = 16 + CH * (1 + 2 * len(glyphs))
    if active:
        add(f'<rect x="{bx + 1:.1f}" y="{BAR_Y + 5}" width="{bw:.1f}" height="{BAR_H - 10}" rx="7" fill="{WS_ON}"/>')
    color = BRIGHT if active else BAR_FG
    text(bx + 8, MID, n, SIZE, color, 400, extra=' fill-opacity=".55"')
    for k, g in enumerate(glyphs):
        gx = bx + 8 + CH * (1.5 + 2 * k) + 2
        add(f'<g transform="translate({gx:.1f},{CY}) scale(.78) translate({-gx:.1f},{-CY})">')
        g(gx, CY, color)
        add("</g>")
    bx += bw + 2
x += ws_w + 6
title = "Meet - Retro"
tw_ = 24 + CH * (2 + len(title))
pill(x, tw_)
w_chrome(x + 12 + CH / 2, CY, BRIGHT)
text(x + 12 + 2 * CH, MID, title, SIZE, BRIGHT)

# Tray: Slack, Discord, Bluetooth.
pill(TRAY_X, TRAY_W)
w_slack(TRAY_X + 19, CY, "#e01e5a")
tx = TRAY_X + 43
add(f'<path d="M{tx - 7},{CY + 4} q-1,-7 2.6,-10 q4.4,-1.6 8.8,0 q3.6,3 2.6,10 q-2,2 -4,2.4 l-1,-1.6 q-2,.6 -4,0 l-1,1.6 q-2,-.4 -4,-2.4 Z" fill="#7289da"/>'
    f'<circle cx="{tx - 2.4}" cy="{CY}" r="1.3" fill="#25272b"/><circle cx="{tx + 2.4}" cy="{CY}" r="1.3" fill="#25272b"/>')
tx = TRAY_X + 67
add(f'<path d="M{tx - 4},{CY - 3.5} l8,7 l-4,3.5 v-14 l4,3.5 l-8,7" fill="none" stroke="{BLUE}" stroke-width="1.6" stroke-linejoin="round"/>')

# Timetable's clock, purple, moving with the skips.
pill(CLOCK_X, CLOCK_W)
for t0, t1, hhmm in steps(TIMES, T):
    group([(t0, t1)], 0.03)
    text(CLOCK_X + 12, MID, f"Thu Nov 12  {hhmm}", SIZE, PURPLE)
    add("</g>")


def lamp_glyph(kind, cx, color, cls=""):
    c = f' class="{cls}"' if cls else ""
    if kind == "mic":
        add(f"<g{c}>")
        g_mic(pen, cx, CY, color)
        add("</g>")
    elif kind == "rec":
        add(f'<circle{c} cx="{cx}" cy="{CY}" r="5.2" fill="{color}"/>')
    else:  # an hourglass
        add(f'<path{c} d="M{cx - 4.5},{CY - 6.5} h9 M{cx - 4.5},{CY + 6.5} h9 M{cx - 3.5},{CY - 6.5} q0,4 3.5,6.5 q3.5,-2.5 3.5,-6.5 M{cx - 3.5},{CY + 6.5} '
            f'q0,-4 3.5,-6.5 q3.5,2.5 3.5,6.5" fill="none" stroke="{color}" stroke-width="1.5" stroke-linecap="round"/>')


# The desk pill: Logbook's lamp, then the drawer's handle. Its right edge
# stays put; a wider lamp pushes every pill to its left along.
frames = steps(LAMPS_FRAMES, T)
widths = {}
for t0, t1, state, words in frames:
    widths.setdefault(desk_w(words), []).append((t0, t1))
for w, sp in widths.items():
    group(sp, 0.02)
    pill(DESK_R - w, w)
    hx = DESK_R - 4 - 16 - CH
    add(f'<path d="M{hx + 8 + CH / 2 + 2:.1f},{CY - 5} l-5,5 l5,5" fill="none" stroke="{BAR_DIM}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>')
    add("</g>")
LAMP_AT = {}  # where the lamp module sits in each state: (left, width)
hover = fade([(CLICK_LAMP - 0.15, CLICK_LAMP + 0.5)], 0.1)
for t0, t1, state, words in frames:
    kind, _, _, color = LAMP_TEXT[state]
    lx = DESK_R - desk_w(words) + 4
    LAMP_AT[t0] = (lx, lamp_w(words))
    group([(t0, t1)], 0.02)
    if t0 <= CLICK_LAMP < t1:
        add(f'<rect x="{lx:.1f}" y="{BAR_Y + 3}" width="{lamp_w(words):.1f}" height="{BAR_H - 6}" rx="7" fill="{HOVER}" {style(hover)} opacity="0"/>')
    lamp_glyph(kind, lx + 8 + CH / 2, color)
    if words:
        text(lx + 8 + 2 * CH, MID, words, SIZE, color)
    add("</g>")

# Everything left of the desk pill shifts with its width.
push_pts = []
base_w = desk_w("")
for t0, t1, state, words in frames:
    v = f"translateX({base_w - desk_w(words):.1f}px)"
    push_pts += [(t0, v), (max(t0, t1 - 0.001), v)]
still_shift = next(base_w - desk_w(w) for t0, t1, s, w in frames if t0 <= STILL < t1)
push = anim("transform", push_pts)
add(f'<g {style(push)} transform="translate({still_shift:.1f},0)">')
x = DESK_R - base_w - 6
# Audio: the volume and the mouse's battery.
AUDIO_W = 4 + 16 + 4 * CH + 16 + 5 * CH + 4
x -= AUDIO_W
pill(x, AUDIO_W)
sx = x + 4 + 8 + CH / 2
add(f'<path d="M{sx - 5},{CY - 2.5} h3 l4,-4 v13 l-4,-4 h-3 Z" fill="{FROST}"/><path d="M{sx + 3},{CY - 3} q2.5,3 0,6" fill="none" stroke="{FROST}" stroke-width="1.4" stroke-linecap="round"/>')
text(x + 4 + 8 + 2 * CH, MID, "35%", SIZE, FROST)
mx = x + 4 + 16 + 4 * CH + 8 + CH / 2
add(f'<rect x="{mx - 4.5}" y="{CY - 7}" width="9" height="14" rx="4.5" fill="{BAR_DIM}"/><path d="M{mx},{CY - 7} v5 M{mx - 4.5},{CY - 2} h9" stroke="#25272b" stroke-width="1.2"/>')
text(mx + CH * 1.5, MID, "80%", SIZE, BAR_DIM)
# Lamps: Roundhouse, Switchyard, Signal Box, VPN, do not disturb.
LAMPS_W = 4 + 5 * (16 + CH) + 4
x -= 6 + LAMPS_W
pill(x, LAMPS_W)
lx = [x + 4 + 8 + CH / 2 + k * (16 + CH) for k in range(5)]
add(f'<circle cx="{lx[0]:.1f}" cy="{CY}" r="6" fill="{GREEN}"/>')
c = lx[1]
add(f'<path d="M{c - 6},{CY - 3} h3 a2,2 0 0 1 4,0 h3 v3 a2,2 0 0 1 0,4 v3 h-3 a2,2 0 0 0 -4,0 h-3 v-3 a2,2 0 0 0 0,-4 Z" fill="{FROST}" transform="translate(-1,0)"/>')
add(f'<circle cx="{lx[2]:.1f}" cy="{CY}" r="5.5" fill="none" stroke="{BAR_DIM}" stroke-width="1.6"/>')
c = lx[3]
add(f'<g fill="none" stroke="{BAR_DIM}" stroke-width="1.4"><rect x="{c - 6:.1f}" y="{CY - 7}" width="6" height="4.5" rx="1"/><rect x="{c + 0.5:.1f}" y="{CY + 2}" width="6" height="4.5" rx="1"/>'
    f'<path d="M{c - 3:.1f},{CY - 2.5} v6.8 h3.5 M{c - 6:.1f},{CY + 1} l2.4,2.4 m0,-2.4 l-2.4,2.4"/></g>')
c = lx[4]


def bell(c, color, off):
    add(f'<path d="M{c - 5.5:.1f},{CY + 3.5} v-4.5 q0,-5.5 5.5,-5.5 q5.5,0 5.5,5.5 v4.5 l1.5,1.5 h-14 Z" fill="{color}"/><circle cx="{c:.1f}" cy="{CY + 7}" r="1.8" fill="{color}"/>')
    if off:
        add(f'<line x1="{c - 7.5:.1f}" y1="{CY - 7.5}" x2="{c + 7.5:.1f}" y2="{CY + 7.5}" stroke="#25272b" stroke-width="3.6"/>'
            f'<line x1="{c - 7.5:.1f}" y1="{CY - 7.5}" x2="{c + 7.5:.1f}" y2="{CY + 7.5}" stroke="{color}" stroke-width="1.7" stroke-linecap="round"/>')


free = [(0, DND[0][0]), (DND[0][1], T)]
group(free)
bell(c, BAR_DIM, False)
add("</g>")
group(DND)
bell(c, YELLOW, True)
add("</g>")
# Tannoy: the cover and the song, or a dim note while do not disturb holds.
MEDIA_R = x - 6
mw = 10 + 18 + 8 + len(SONG) * CH + 12
group(free)
pill(MEDIA_R - mw, mw)
cx0 = MEDIA_R - mw + 10
add(f'<rect x="{cx0:.1f}" y="{CY - 9}" width="18" height="18" rx="3" fill="url(#dusk)"/><circle cx="{cx0 + 11:.1f}" cy="{CY - 2}" r="3" fill="{YELLOW}"/>'
    f'<path d="M{cx0:.1f},{CY + 5} q5,-3 9,0 t9,0 v4 h-18 Z" fill="#2e2235"/>')
text(cx0 + 26, MID, SONG, SIZE, YELLOW)
add("</g>")
group(DND)
pill(MEDIA_R - 24 - CH, 24 + CH)
nx = MEDIA_R - 12 - CH / 2
add(f'<g transform="translate({nx:.1f},{CY}) scale(.9)" fill="{BAR_DIM}"><ellipse cx="-4.2" cy="5" rx="2.9" ry="2.3"/><ellipse cx="4.6" cy="3.2" rx="2.9" ry="2.3"/>'
    '<rect x="-1.9" y="-6.5" width="1.6" height="11.5"/><rect x="6.9" y="-8.3" width="1.6" height="11.5"/><path d="M-1.9,-6.5 L8.5,-8.3 L8.5,-5.6 L-1.9,-3.8 Z"/></g>')
add("</g>")
add("</g>")

# ---------------------------------------------------------------- the drawer

# It opens out of the lamp where it was clicked, anchored to the bar, and
# keeps that place while the lamp's width changes under it.
lx, lw = next(v for t0, v in sorted(LAMP_AT.items(), reverse=True) if t0 <= CLICK_LAMP)
LAMP_C = (lx + lw / 2, CY)
DX = min(max(LAMP_C[0] - DRAWER_W / 2, 12), W - DRAWER_W - 12)


def top(s):
    return BAR_Y - 8 - drawer_size(s)[1]


spring = anim("transform", [
    (0, "translateY(26px) scale(.06)"), (OPEN - 0.01, "translateY(26px) scale(.06)"),
    (OPEN + 0.10, "translateY(14px) scale(.5)"), (OPEN + 0.20, "translateY(4px) scale(.9)"),
    (OPEN + 0.30, "translateY(-1px) scale(1.035)"), (OPEN + 0.42, "translateY(0) scale(.988)"),
    (OPEN + 0.56, "scale(1.006)"), (OPEN + 0.72, "scale(1)"), (CLICK_OUT + 0.08, "scale(1)"),
    (CLOSED, "translateY(14px) scale(.4)"), (T, "translateY(14px) scale(.4)"),
])
show = anim("opacity", [(0, 0), (OPEN - 0.01, 0), (OPEN + 0.12, 0.85), (OPEN + 0.2, 1), (CLICK_OUT + 0.08, 1), (CLOSED, 0), (T, 0)])
add(f'<g {style(spring, show, extra=f"transform-origin:{LAMP_C[0]:.1f}px {LAMP_C[1]:.1f}px")}>')


def ticking(p, x, y):
    """The drawer's clock hook: the meeting's clock, one second at a time."""
    for t0, t1, c in steps(CLOCKS, T):
        p.add(f'<g {style(fade([(t0, t1)], 0.01))} opacity="{1 if at([(t0, t1)]) else 0}">')
        p.text(x, y, c, 26, LB["rec"], 700)
        p.add("</g>")


SPOTS = []  # (from, to, top, spots) for the clicks
for t0, t1, s in STATES:
    y0 = top(s)
    group([(t0, t1)], 0.02)
    add(f'<g transform="translate({DX:.1f},{y0:.1f})">')
    sp = drawer(pen, s, ticking if s.meeting else None)
    add("</g></g>")
    SPOTS.append((t0, t1, y0, sp))
add("</g>")


def spot(name, t):
    for t0, t1, y0, sp in SPOTS:
        if t0 <= t < t1 and name in sp:
            return DX + sp[name][0], y0 + sp[name][1]
    raise KeyError(name)


# ---------------------------------------------------------------- cursor

record_c = spot("record-meeting", CLICK_RECORD)
mark_c = spot("mark", CLICK_MARK)
stop_c = spot("stop", CLICK_STOP)
copy_c = spot("copy:0", CLICK_COPY)
OUT_C = (470, 150)
clicks = [(CLICK_JOIN, *JOIN_C), (CLICK_LAMP, *LAMP_C), (CLICK_RECORD, *record_c), (CLICK_MARK, *mark_c),
          (CLICK_STOP, *stop_c), (CLICK_COPY, *copy_c), (CLICK_OUT, *OUT_C), (CLICK_LEAVE, *LEAVE_C)]
for t, cx_, cy_ in clicks:
    k = anim("opacity", [(0, 0), (t - 0.01, 0), (t, 0.9), (t + 0.45, 0), (T, 0)])
    k2 = anim("transform", [(0, "scale(.3)"), (t, "scale(.3)"), (t + 0.45, "scale(1.6)"), (T, "scale(1.6)")])
    add(f'<circle class="a ripple" style="animation-name:{k},{k2};transform-box:fill-box;transform-origin:center" cx="{cx_:.1f}" cy="{cy_:.1f}" r="18" fill="none" stroke="{FROST}" stroke-width="2.5" opacity="0"/>')


def tr(p):
    return f"translate({p[0] - 3:.1f}px, {p[1] - 4:.1f}px)"


REST = (360, 300)
cur = anim("transform", [
    (0, tr(REST)), (0.2, tr(REST)), (CLICK_JOIN - 0.15, tr(JOIN_C)), (1.2, tr(JOIN_C)),
    (CLICK_LAMP - 0.15, tr(LAMP_C)), (2.6, tr(LAMP_C)),
    (CLICK_RECORD - 0.15, tr(record_c)), (4.2, tr(record_c)),
    (CLICK_MARK - 0.3, tr(mark_c)), (7.3, tr(mark_c)),
    (CLICK_STOP - 0.2, tr(stop_c)), (9.8, tr(stop_c)),
    (CLICK_COPY - 0.2, tr(copy_c)), (12.3, tr(copy_c)),
    (CLICK_OUT - 0.15, tr(OUT_C)), (13.0, tr(OUT_C)),
    (CLICK_LEAVE - 0.15, tr(LEAVE_C)), (14.3, tr(LEAVE_C)),
    (RESET + 0.5, tr(REST)), (T, tr(REST)),
])
add(f'<g id="cursor" class="a" style="animation-name:{cur};animation-timing-function:cubic-bezier(.45,0,.25,1);transform:{tr(REST)}">')
add('<path d="M0,0 L0,22 L6,17 L10,26 L14,24 L10,15 L18,15 Z" fill="#eceff4" stroke="#1d2024" stroke-width="1.4" stroke-linejoin="round"/>')
add("</g>")
add("</g></svg>")

out[STYLE_AT] = "<style>" + css + "\n".join(kfs) + "</style>"
Path(__file__).with_name("hero.svg").write_text("\n".join(out) + "\n")
print(f"wrote hero.svg ({sum(len(s) + 1 for s in out) // 1024} KB, {len(kfs)} keyframe sets)")
