//! The drawer: what is recording now (with its clock, level meter, Stop
//! and Mark), who else has the mic, the transcript being made, and the
//! recordings and transcripts on disk. Three tabs: Now, Recordings,
//! Transcripts.

use crate::anim::{Motion, Spring};
use crate::meter::Meter;
use crate::stage::Stage;
use bar_anchor::Anchor;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge as LsEdge, KeyboardMode, Layer, LayerShell};
use logbook_core::config::Config;
use logbook_core::model::{
    Dictation, Job, JobState, Kind, Lamp, Recording, ScreenPhase, Snapshot, Transcript,
};
use logbook_core::protocol::{Event, Request, What};
use logbook_core::time::{clock, length, when};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

pub enum Msg {
    Event(Event),
    /// A watched stylesheet or the config changed.
    Restyle,
}

const MIC: &str = "\u{f036c}";
const PEOPLE: &str = "\u{f0849}";
const SCREEN: &str = "\u{f0379}";
const COPY: &str = "\u{f018f}";
const FOLDER: &str = "\u{f0770}";
const STOP: &str = "\u{f04db}";
const FLAG: &str = "\u{f023b}";
const PLAY: &str = "\u{f040a}";
const DOC: &str = "\u{f0219}";
const RETRY: &str = "\u{f0450}";
const CLOSE: &str = "\u{f0156}";

const TABS: [&str; 3] = ["Now", "Recordings", "Transcripts"];

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn button(glyph: &str, text: &str, classes: &[&str]) -> gtk::Button {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if !glyph.is_empty() {
        row.append(&label(glyph, &["glyph"]));
    }
    row.append(&label(text, &[]));
    let b = gtk::Button::new();
    b.set_child(Some(&row));
    b.set_focus_on_click(false);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn lab_row(left: &str, right: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let l = label(&left.to_uppercase(), &["t-label"]);
    l.set_hexpand(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&l);
    if !right.is_empty() {
        row.append(&label(&right.to_uppercase(), &["t-label"]));
    }
    row
}

fn card(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 7);
    b.add_css_class("card");
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn send_quiet(req: Request) {
    std::thread::spawn(move || {
        let _ = logbook_core::protocol::request(&req);
    });
}

/// The `logbook` binary: next to the drawer, else on PATH.
fn logbook_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("logbook")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("logbook"))
}

fn run(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) {
    let child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut c) = child {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

fn copy_file(path: String) {
    std::thread::spawn(move || {
        let Ok(text) = std::fs::read(&path) else {
            return;
        };
        let child = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut c) = child {
            use std::io::Write;
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(&text);
            }
            let _ = c.wait();
        }
    });
}

fn find_monitor(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let list = display.monitors();
    (0..list.n_items())
        .filter_map(|i| list.item(i).and_downcast::<gdk::Monitor>())
        .find(|m| m.connector().is_some_and(|c| c == name))
}

fn set_state_class(w: &impl IsA<gtk::Widget>, state: &str) {
    for c in [
        "recording",
        "dictating",
        "processing",
        "transcribing",
        "mic",
        "off",
    ] {
        w.remove_css_class(c);
    }
    w.add_css_class(state);
}

fn file_name(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn plural(n: u32, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// A label that shows the time since `since`, kept current by the tick.
struct Clock {
    label: gtk::Label,
    since: f64,
}

pub struct Drawer {
    window: gtk::ApplicationWindow,
    panel: gtk::Box,
    stage: Stage,
    spring: Spring,
    visible: Cell<bool>,
    snap: RefCell<Option<Snapshot>>,
    cfg: RefCell<Config>,
    /// The daemon's clock minus ours, so clocks match the bar's.
    skew: Cell<f64>,

    lamp: gtk::Label,
    sub: gtk::Label,
    chip: gtk::Label,
    notice: gtk::Label,
    tabs: Vec<gtk::ToggleButton>,
    pages: gtk::Stack,
    tab: Cell<usize>,

    live: gtk::Box,
    others: gtk::Box,
    work: gtk::Box,
    recent: gtk::Box,
    recordings: gtk::Box,
    transcripts: gtk::Box,
    /// What each part was last built from, so a snapshot that only moves
    /// the clock does not rebuild it.
    sigs: RefCell<[String; 6]>,
    clocks: RefCell<Vec<Clock>>,
    meter: Meter,
    meter_path: RefCell<Option<String>>,
    eta: RefCell<Option<(gtk::Label, Job)>>,
}

impl Drawer {
    pub fn new(app: &gtk::Application, _tx: async_channel::Sender<Msg>) -> Rc<Drawer> {
        let (cfg, _) = Config::load();
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("logbook");
        window.set_decorated(false);
        window.set_title(Some("Logbook"));
        // Without layer-shell (another backend, `--render`) it is a plain
        // window.
        if crate::linux::layer_shell() {
            window.init_layer_shell();
            window.set_layer(Layer::Overlay);
            window.set_namespace(Some("logbook"));
            for e in [LsEdge::Top, LsEdge::Bottom, LsEdge::Left, LsEdge::Right] {
                window.set_anchor(e, true);
            }
            window.set_exclusive_zone(0);
            window.set_keyboard_mode(KeyboardMode::Exclusive);
        } else {
            window.set_default_size(cfg.drawer.width as i32 + 80, 760);
        }

        // ---- header
        let lamp = label("\u{25cf}", &["lamp", "off"]);
        lamp.set_valign(gtk::Align::Center);
        let brand = label("LOGBOOK", &["brand"]);
        let sub = label("", &["sub"]);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let who = gtk::Box::new(gtk::Orientation::Vertical, 0);
        who.append(&brand);
        who.append(&sub);
        who.set_hexpand(true);
        let chip = label("off air", &["chip", "off"]);
        chip.set_valign(gtk::Align::Center);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.append(&lamp);
        header.append(&who);
        header.append(&chip);

        let notice = label("", &["notice"]);
        notice.set_wrap(true);
        notice.set_visible(false);

        // ---- tabs
        let tabbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabbar.add_css_class("tabs");
        let mut tabs: Vec<gtk::ToggleButton> = vec![];
        for (i, name) in TABS.iter().enumerate() {
            let t = gtk::ToggleButton::with_label(name);
            t.set_focus_on_click(false);
            if let Some(first) = tabs.first() {
                t.set_group(Some(first));
            }
            t.set_active(i == 0);
            tabbar.append(&t);
            tabs.push(t);
        }

        // ---- pages
        let live = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let others = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let work = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let recent = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let now_page = gtk::Box::new(gtk::Orientation::Vertical, 10);
        now_page.append(&live);
        now_page.append(&others);
        now_page.append(&work);
        now_page.append(&recent);

        let recordings = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let transcripts = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let scroll = |child: &gtk::Box| {
            let s = gtk::ScrolledWindow::new();
            s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            s.set_propagate_natural_height(true);
            s.set_max_content_height(460);
            s.set_child(Some(child));
            s
        };
        let pages = gtk::Stack::new();
        pages.set_vhomogeneous(false);
        pages.set_interpolate_size(true);
        pages.set_transition_type(gtk::StackTransitionType::Crossfade);
        pages.set_transition_duration(140);
        pages.add_named(&scroll(&now_page), Some("0"));
        pages.add_named(&scroll(&recordings), Some("1"));
        pages.add_named(&scroll(&transcripts), Some("2"));

        let foot = label(
            "1-3 tabs \u{b7} r record meeting \u{b7} m mark \u{b7} s screen \u{b7} Esc close",
            &["foot"],
        );

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
        panel.add_css_class("panel");
        panel.append(&header);
        panel.append(&notice);
        panel.append(&tabbar);
        panel.append(&pages);
        panel.append(&foot);

        let spring = Spring::new(&panel, Motion::PANEL);
        let stage = Stage::new(
            &spring,
            cfg.drawer.width,
            cfg.drawer.max_height,
            cfg.drawer.gap,
        );
        window.set_child(Some(&stage));

        let d = Rc::new(Drawer {
            window,
            panel: panel.clone(),
            stage,
            spring,
            visible: Cell::new(false),
            snap: RefCell::new(None),
            cfg: RefCell::new(cfg),
            skew: Cell::new(0.0),
            lamp,
            sub,
            chip,
            notice,
            tabs,
            pages,
            tab: Cell::new(0),
            live,
            others,
            work,
            recent,
            recordings,
            transcripts,
            sigs: RefCell::new(Default::default()),
            clocks: RefCell::new(vec![]),
            meter: Meter::new(),
            meter_path: RefCell::new(None),
            eta: RefCell::new(None),
        });
        d.connect();
        d
    }

    fn connect(self: &Rc<Self>) {
        {
            let w = Rc::downgrade(self);
            self.stage.connect_outside(move || {
                if let Some(d) = w.upgrade() {
                    d.hide();
                }
            });
            let s = self.spring.clone();
            self.stage.connect_placed(move |(x, y)| s.set_origin(x, y));
        }
        for (i, t) in self.tabs.iter().enumerate() {
            let w = Rc::downgrade(self);
            t.connect_toggled(move |t| {
                if t.is_active() {
                    if let Some(d) = w.upgrade() {
                        d.select(i);
                    }
                }
            });
        }
        {
            let keys = gtk::EventControllerKey::new();
            let w = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, _| {
                let Some(d) = w.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                d.key(key)
            });
            self.window.add_controller(keys);
        }
        // Clocks and the level meter move between snapshots.
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(d) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if d.visible.get() {
                d.tick();
            }
            glib::ControlFlow::Continue
        });
    }

    /// Opens on the given tab, waits for the daemon's snapshot and the
    /// open animation, writes the panel to a PNG and quits. A development
    /// aid: run it under `GDK_BACKEND=broadway` to see the drawer without a
    /// compositor.
    pub fn render_to(self: &Rc<Self>, path: PathBuf, tab: usize, app: gtk::Application) {
        // Nothing draws a headless display, so its frame clock never ticks
        // and any animation would stay at its first frame.
        if let Some(s) = gtk::Settings::default() {
            s.set_gtk_enable_animations(false);
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(600), move || {
            let Some(d) = w.upgrade() else { return };
            d.show(None);
            // Switching pages makes GTK produce the panel's first frame,
            // which a headless display would otherwise never ask for.
            d.select((tab + 1) % TABS.len());
            d.select(tab);
            let tries = Rc::new(Cell::new(0u32));
            let w = Rc::downgrade(&d);
            glib::timeout_add_local(Duration::from_millis(400), move || {
                let Some(d) = w.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                d.spring.settle();
                let p = gtk::WidgetPaintable::new(Some(&d.panel));
                let (pw, ph) = (d.panel.width() as f64, d.panel.height() as f64);
                let snap = gtk::Snapshot::new();
                p.snapshot(&snap, pw, ph);
                // The first frames of a headless display can come back
                // empty; try again for a few seconds.
                let Some(node) = snap.to_node() else {
                    tries.set(tries.get() + 1);
                    if tries.get() < 15 {
                        d.window.queue_draw();
                        return glib::ControlFlow::Continue;
                    }
                    eprintln!("logbook-board: could not render the panel: nothing to draw");
                    app.quit();
                    return glib::ControlFlow::Break;
                };
                // A Cairo renderer needs no surface, so this works on any
                // backend.
                let r = gtk::gsk::CairoRenderer::new();
                let saved = r
                    .realize(None::<&gdk::Surface>)
                    .map_err(|e| e.to_string())
                    .and_then(|_| {
                        let t = r.render_texture(node, None);
                        r.unrealize();
                        t.save_to_png(&path).map_err(|e| e.to_string())
                    });
                match saved {
                    Ok(()) => eprintln!("logbook-board: wrote {}", path.display()),
                    Err(e) => eprintln!("logbook-board: could not render the panel: {e}"),
                }
                app.quit();
                glib::ControlFlow::Break
            });
        });
    }

    fn select(&self, i: usize) {
        self.tab.set(i);
        if let Some(t) = self.tabs.get(i) {
            if !t.is_active() {
                t.set_active(true);
            }
        }
        self.pages.set_visible_child_name(&i.to_string());
    }

    pub fn handle(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Event(Event::Snapshot { snapshot }) => {
                self.skew.set(snapshot.now - logbook_core::now());
                *self.snap.borrow_mut() = Some(*snapshot);
                if self.visible.get() {
                    self.render();
                }
            }
            Msg::Event(Event::Show { anchor }) => self.show(anchor),
            Msg::Event(Event::Toggle { anchor }) => {
                if self.visible.get() {
                    self.hide();
                } else {
                    self.show(anchor);
                }
            }
            Msg::Event(Event::Hide) => self.hide(),
            Msg::Restyle => {
                crate::linux::apply_styles();
                let (cfg, _) = Config::load();
                self.stage
                    .set_geometry(cfg.drawer.width, cfg.drawer.max_height, cfg.drawer.gap);
                *self.cfg.borrow_mut() = cfg;
                *self.sigs.borrow_mut() = Default::default();
                if self.visible.get() {
                    self.render();
                }
            }
        }
    }

    fn key(self: &Rc<Self>, key: gdk::Key) -> glib::Propagation {
        let snap = self.snap.borrow().clone();
        match key {
            gdk::Key::Escape => self.hide(),
            gdk::Key::_1 | gdk::Key::KP_1 => self.select(0),
            gdk::Key::_2 | gdk::Key::KP_2 => self.select(1),
            gdk::Key::_3 | gdk::Key::KP_3 => self.select(2),
            gdk::Key::Tab => self.select((self.tab.get() + 1) % TABS.len()),
            gdk::Key::ISO_Left_Tab => self.select((self.tab.get() + TABS.len() - 1) % TABS.len()),
            gdk::Key::r => {
                if snap.as_ref().is_some_and(|s| s.meeting.is_some()) {
                    send_quiet(Request::Stop {
                        what: Some(What::Meeting),
                    });
                } else {
                    send_quiet(Request::Start {
                        what: What::Meeting,
                        geometry: None,
                    });
                }
            }
            gdk::Key::m => send_quiet(Request::Mark),
            gdk::Key::s => {
                if snap.as_ref().is_some_and(|s| s.screen.is_some()) {
                    send_quiet(Request::Stop {
                        what: Some(What::Screen),
                    });
                } else {
                    self.record_screen();
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    fn show(self: &Rc<Self>, anchor: Option<Anchor>) {
        if let Some(name) = anchor.as_ref().and_then(|a| a.monitor.clone()) {
            if let Some(m) = find_monitor(&name) {
                self.window.set_monitor(Some(&m));
            }
        }
        self.stage.set_anchor(anchor);
        let was_visible = self.visible.replace(true);
        if !was_visible {
            self.select(0);
        }
        self.render();
        self.window.set_visible(true);
        self.window.present();
        if !was_visible {
            self.spring.play_in(0);
            send_quiet(Request::BoardVisible { visible: true });
        }
    }

    fn hide(self: &Rc<Self>) {
        if !self.visible.replace(false) {
            return;
        }
        let w = Rc::downgrade(self);
        self.spring.play_out(move || {
            if let Some(d) = w.upgrade() {
                if !d.visible.get() {
                    d.window.set_visible(false);
                }
            }
        });
        send_quiet(Request::BoardVisible { visible: false });
    }

    /// slurp needs the screen: close first, then let `logbook start
    /// screen` ask for the region.
    fn record_screen(self: &Rc<Self>) {
        self.hide();
        glib::timeout_add_local_once(Duration::from_millis(350), || {
            run(logbook_bin(), &["start", "screen"]);
        });
    }

    fn dictate(self: &Rc<Self>) {
        self.hide();
        run("voxtype", &["record", "toggle"]);
    }

    fn now(&self) -> f64 {
        logbook_core::now() + self.skew.get()
    }

    fn tick(&self) {
        let t = self.now();
        for c in self.clocks.borrow().iter() {
            c.label.set_label(&clock(t - c.since));
        }
        if let Some((l, j)) = self.eta.borrow().as_ref() {
            l.set_label(&match j.eta(t) {
                Some(s) if s >= 60.0 => format!("~{} min left", (s / 60.0).round() as u64),
                Some(_) => "under a minute left".into(),
                None => String::new(),
            });
        }
        let path = self.meter_path.borrow().clone();
        match path {
            Some(p) => self
                .meter
                .push(logbook_core::media::tail_level(Path::new(&p), 0.1).unwrap_or(0.0)),
            None => self.meter.clear(),
        }
    }

    fn bind(self: &Rc<Self>, b: &gtk::Button, f: impl Fn(&Rc<Drawer>) + 'static) {
        let w = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(d) = w.upgrade() {
                f(&d);
            }
        });
    }

    fn render(self: &Rc<Self>) {
        let snap = self.snap.borrow().clone();
        let Some(s) = snap else {
            self.sub.set_label("waiting for the daemon");
            return;
        };
        match (&s.config_error, &s.problem) {
            (Some(e), _) => {
                self.notice
                    .set_label(&format!("Config problem, using defaults: {e}"));
                self.notice.set_visible(true);
            }
            (None, Some(p)) => {
                self.notice.set_label(p);
                self.notice.set_visible(true);
            }
            _ => self.notice.set_visible(false),
        }
        let state = s.lamp.as_str();
        set_state_class(&self.lamp, state);
        set_state_class(&self.chip, state);
        self.chip.set_label(match s.lamp {
            Lamp::Recording => "on air",
            Lamp::Dictating => "listening",
            Lamp::Processing => "saving",
            Lamp::Transcribing => "transcribing",
            Lamp::Mic => "mic in use",
            Lamp::Off => "off air",
        });
        let t = self.now();
        let today = s
            .recordings
            .iter()
            .filter(|r| when(r.started, t).starts_with("today"))
            .count() as u32;
        let cfg = self.cfg.borrow().clone();
        self.sub.set_label(&format!(
            "{} today \u{b7} {}",
            plural(today, "recording"),
            logbook_core::paths::tilde(&cfg.meeting_dir())
                .rsplit_once('/')
                .map(|(a, _)| a.to_string())
                .unwrap_or_else(|| "~".into())
        ));

        self.render_live(&s);
        self.render_others(&s);
        self.render_work(&s);
        self.render_recent(&s);
        self.render_recordings(&s);
        self.render_transcripts(&s);
        self.tick();
    }

    fn render_live(self: &Rc<Self>, s: &Snapshot) {
        let sig = format!(
            "{:?}|{:?}|{:?}|{}",
            s.meeting
                .as_ref()
                .map(|m| (&m.dir, m.chunk, m.marks.len(), m.started.to_bits())),
            s.screen.as_ref().map(|x| (&x.path, x.phase)),
            s.dictation,
            s.dnd_held
        );
        if self.sigs.borrow()[0] == sig {
            return;
        }
        self.sigs.borrow_mut()[0] = sig;
        clear(&self.live);
        let mut clocks = vec![];
        *self.meter_path.borrow_mut() = None;

        if let Some(m) = &s.meeting {
            let c = card(&["live", "rec"]);
            c.append(&lab_row(
                &format!("Meeting \u{b7} chunk {} \u{b7} {}", m.chunk, m.source),
                if s.dnd_held { "dnd on" } else { "" },
            ));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let big = label(&clock(self.now() - m.started), &["big", "rec"]);
            big.set_hexpand(true);
            clocks.push(Clock {
                label: big.clone(),
                since: m.started,
            });
            row.append(&big);
            if let Some(p) = self.meter.area.parent() {
                if let Ok(b) = p.downcast::<gtk::Box>() {
                    b.remove(&self.meter.area);
                }
            }
            row.append(&self.meter.area);
            *self.meter_path.borrow_mut() = Some(m.chunk_path.clone());
            c.append(&row);
            if let Some(last) = m.marks.last() {
                c.append(&label(
                    &format!(
                        "{} \u{b7} last at {}",
                        plural(m.marks.len() as u32, "mark"),
                        clock(*last)
                    ),
                    &["dim"],
                ));
            }
            let btns = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            btns.add_css_class("btns");
            let stop = button(STOP, "Stop", &["danger"]);
            self.bind(&stop, |_| {
                send_quiet(Request::Stop {
                    what: Some(What::Meeting),
                })
            });
            let mark = button(FLAG, "Mark", &[]);
            mark.set_tooltip_text(Some("A heading in the transcript at this moment (m)"));
            self.bind(&mark, |_| send_quiet(Request::Mark));
            btns.append(&stop);
            btns.append(&mark);
            c.append(&btns);
            self.live.append(&c);
        }
        if let Some(x) = &s.screen {
            let c = card(&["live", "rec"]);
            match x.phase {
                ScreenPhase::Recording => {
                    c.append(&lab_row(
                        &format!("Screen \u{b7} {}", x.geometry),
                        if s.dnd_held { "dnd on" } else { "" },
                    ));
                    let big = label(&clock(self.now() - x.started), &["big", "rec"]);
                    clocks.push(Clock {
                        label: big.clone(),
                        since: x.started,
                    });
                    c.append(&big);
                    let btns = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    btns.add_css_class("btns");
                    let stop = button(STOP, "Stop", &["danger"]);
                    self.bind(&stop, |_| {
                        send_quiet(Request::Stop {
                            what: Some(What::Screen),
                        })
                    });
                    btns.append(&stop);
                    c.append(&btns);
                }
                ScreenPhase::Processing => {
                    c.remove_css_class("rec");
                    c.add_css_class("busy");
                    c.append(&lab_row("Screen \u{b7} saving", "loudness pass"));
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let sp = gtk::Spinner::new();
                    sp.start();
                    row.append(&sp);
                    row.append(&label(&file_name(&x.path), &[]));
                    c.append(&row);
                }
            }
            self.live.append(&c);
        }
        if matches!(s.dictation, Dictation::Recording | Dictation::Transcribing) {
            let c = card(&["live", "rec"]);
            let typing = s.dictation == Dictation::Transcribing;
            c.append(&lab_row(
                "Dictation \u{b7} voxtype",
                if typing { "typing" } else { "listening" },
            ));
            c.append(&label(
                if typing {
                    "Typing out what you said\u{2026}"
                } else {
                    "Release the key, or press Stop, to type it."
                },
                &["dim"],
            ));
            if !typing {
                let btns = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                btns.add_css_class("btns");
                let stop = button(STOP, "Stop", &["danger"]);
                self.bind(&stop, |_| run("voxtype", &["record", "stop"]));
                btns.append(&stop);
                c.append(&btns);
            }
            self.live.append(&c);
        }
        if self.live.first_child().is_none() {
            let c = card(&["live"]);
            c.append(&lab_row(
                "Mic",
                if s.others().next().is_some() {
                    "in use"
                } else {
                    "closed"
                },
            ));
            let text = if s.others().next().is_some() {
                "Logbook is not recording."
            } else if s.mic_known {
                "Nothing is listening. No app has the mic open."
            } else {
                "Logbook is not recording. (PipeWire is not answering, so other apps are unknown.)"
            };
            let t = label(text, &[]);
            t.set_wrap(true);
            c.append(&t);
            let btns = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            btns.add_css_class("btns");
            let meet = button(PEOPLE, "Record meeting", &["primary"]);
            self.bind(&meet, |_| {
                send_quiet(Request::Start {
                    what: What::Meeting,
                    geometry: None,
                })
            });
            let screen = button(SCREEN, "Record screen", &[]);
            self.bind(&screen, |d| d.record_screen());
            btns.append(&meet);
            btns.append(&screen);
            if s.dictation != Dictation::Stopped {
                let dict = button(MIC, "Dictate", &[]);
                self.bind(&dict, |d| d.dictate());
                btns.append(&dict);
            }
            c.append(&btns);
            self.live.append(&c);
        }
        *self.clocks.borrow_mut() = clocks;
    }

    fn render_others(&self, s: &Snapshot) {
        let others: Vec<_> = s.others().collect();
        let sig = format!("{others:?}");
        if self.sigs.borrow()[1] == sig {
            return;
        }
        self.sigs.borrow_mut()[1] = sig;
        clear(&self.others);
        self.others.set_visible(!others.is_empty());
        if others.is_empty() {
            return;
        }
        let c = card(&["others"]);
        let names: Vec<&str> = others.iter().map(|u| u.name.as_str()).collect();
        c.append(&lab_row("Also using the mic", &names.join(" \u{b7} ")));
        for u in others {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&label(MIC, &["glyph", "mic"]));
            let name = if u.app.is_empty() || u.app.eq_ignore_ascii_case(&u.name) {
                u.name.clone()
            } else {
                format!("{} ({})", u.name, u.app)
            };
            let l = label(&name, &[]);
            l.set_hexpand(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&l);
            let src = label(&u.source, &["dim"]);
            src.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&src);
            c.append(&row);
        }
        self.others.append(&c);
    }

    fn render_work(self: &Rc<Self>, s: &Snapshot) {
        let job = s.running_job().cloned();
        let queued = s
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .count();
        let sig = format!("{job:?}|{queued}");
        if self.sigs.borrow()[2] == sig {
            return;
        }
        self.sigs.borrow_mut()[2] = sig;
        clear(&self.work);
        *self.eta.borrow_mut() = None;
        self.work.set_visible(job.is_some());
        let Some(j) = job else {
            return;
        };
        let c = card(&["busy"]);
        let runner = match j.runner.as_deref() {
            Some("switchyard") => "switchyard",
            Some(_) => "lowprio",
            None => "starting",
        };
        let name = s
            .recordings
            .iter()
            .find(|r| r.path == j.dir)
            .map(|r| when(r.started, s.now))
            .unwrap_or_else(|| file_name(&j.dir));
        c.append(&lab_row(
            &format!("Transcribing \u{b7} meeting {name}"),
            runner,
        ));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let what = if j.total > 0 {
            format!("Piece {} of {}", (j.done + 1).min(j.total), j.total)
        } else {
            "Starting".into()
        };
        let w = label(&what, &[]);
        w.set_hexpand(true);
        row.append(&w);
        let eta = label("", &["busy-text"]);
        row.append(&eta);
        c.append(&row);
        let bar = gtk::ProgressBar::new();
        bar.set_fraction(j.fraction());
        bar.add_css_class("prog");
        c.append(&bar);
        if let Some(b) = &j.behind {
            c.append(&lab_row("Queued behind", b));
        }
        if queued > 0 {
            c.append(&label(
                &format!("{} more waiting", plural(queued as u32, "meeting")),
                &["dim"],
            ));
        }
        *self.eta.borrow_mut() = Some((eta, j));
        self.work.append(&c);
    }

    /// One recording as a row with its buttons.
    fn row(self: &Rc<Self>, r: &Recording, now: f64) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("item");
        let (glyph, kind) = match r.kind {
            Kind::Meeting => (PEOPLE, "Meeting"),
            Kind::Screen => (SCREEN, "Screen"),
        };
        row.append(&label(glyph, &["glyph", "kind"]));
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        let title = label(
            &format!("{kind} \u{b7} {}", when(r.started, now)),
            &["title"],
        );
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&title);
        let mut sub = vec![length(r.length)];
        match r.kind {
            Kind::Meeting => {
                if r.marks > 0 {
                    sub.push(plural(r.marks, "mark"));
                }
                sub.push(
                    match r.transcript {
                        Transcript::Ready => "transcript ready",
                        Transcript::Queued => "transcript queued",
                        Transcript::Running => "transcribing",
                        Transcript::Failed => "transcript failed",
                        Transcript::Missing | Transcript::None => "no transcript",
                    }
                    .into(),
                );
            }
            Kind::Screen => sub.push(file_name(&r.path)),
        }
        let s = label(&sub.join(" \u{b7} "), &["dim", "small"]);
        s.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&s);
        row.append(&text);

        let path = r.path.clone();
        match r.kind {
            Kind::Meeting => {
                if let Some(tp) = r.transcript_path.clone() {
                    let b = button(COPY, "Copy", &["small"]);
                    b.set_tooltip_text(Some("Copy the transcript"));
                    self.bind(&b, move |_| copy_file(tp.clone()));
                    row.append(&b);
                } else if matches!(r.transcript, Transcript::Missing | Transcript::Failed) {
                    let b = button(DOC, "Transcribe", &["small"]);
                    let p = path.clone();
                    self.bind(&b, move |_| send_quiet(Request::Queue { dir: p.clone() }));
                    row.append(&b);
                }
                let b = button(FOLDER, "", &["small", "icon"]);
                b.set_tooltip_text(Some("Open the folder"));
                self.bind(&b, move |_| run("xdg-open", &[&path]));
                row.append(&b);
            }
            Kind::Screen => {
                let b = button(PLAY, "Open", &["small"]);
                let p = path.clone();
                self.bind(&b, move |_| run("xdg-open", &[&p]));
                row.append(&b);
                let dir = Path::new(&path)
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default();
                let b = button(FOLDER, "", &["small", "icon"]);
                b.set_tooltip_text(Some("Open the folder"));
                self.bind(&b, move |_| run("xdg-open", &[&dir]));
                row.append(&b);
            }
        }
        row
    }

    fn list_sig(s: &Snapshot, n: usize) -> String {
        let t = logbook_core::now();
        s.recordings
            .iter()
            .take(n)
            .map(|r| {
                format!(
                    "{}:{}:{:?}:{}:{}",
                    r.path,
                    r.length.round(),
                    r.transcript,
                    r.marks,
                    when(r.started, t)
                )
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn render_recent(self: &Rc<Self>, s: &Snapshot) {
        let sig = Self::list_sig(s, 3);
        if self.sigs.borrow()[3] == sig {
            return;
        }
        self.sigs.borrow_mut()[3] = sig;
        clear(&self.recent);
        self.recent.set_visible(!s.recordings.is_empty());
        if s.recordings.is_empty() {
            return;
        }
        self.recent.append(&lab_row(
            "Recent",
            &format!("{} in all", s.recordings.len()),
        ));
        for r in s.recordings.iter().take(3) {
            self.recent.append(&self.row(r, s.now));
        }
    }

    fn render_recordings(self: &Rc<Self>, s: &Snapshot) {
        let sig = Self::list_sig(s, usize::MAX);
        if self.sigs.borrow()[4] == sig {
            return;
        }
        self.sigs.borrow_mut()[4] = sig;
        clear(&self.recordings);
        if s.recordings.is_empty() {
            let c = card(&[]);
            let t = label(
                "No recordings yet. Meetings land in their own folder with 15 minute chunks; screen captures land beside your videos.",
                &[],
            );
            t.set_wrap(true);
            c.append(&t);
            self.recordings.append(&c);
            return;
        }
        let meetings = s
            .recordings
            .iter()
            .filter(|r| r.kind == Kind::Meeting)
            .count();
        self.recordings.append(&lab_row(
            &format!(
                "{} \u{b7} {}",
                plural(meetings as u32, "meeting"),
                plural((s.recordings.len() - meetings) as u32, "capture")
            ),
            "newest first",
        ));
        for r in &s.recordings {
            self.recordings.append(&self.row(r, s.now));
        }
    }

    fn render_transcripts(self: &Rc<Self>, s: &Snapshot) {
        let sig = format!("{:?}|{}", s.jobs, Self::list_sig(s, usize::MAX));
        if self.sigs.borrow()[5] == sig {
            return;
        }
        self.sigs.borrow_mut()[5] = sig;
        clear(&self.transcripts);
        let failed: Vec<&Job> = s
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Failed)
            .collect();
        let waiting: Vec<&Job> = s
            .jobs
            .iter()
            .filter(|j| j.state != JobState::Failed)
            .collect();
        if !waiting.is_empty() {
            self.transcripts.append(&lab_row(
                "In the works",
                &plural(waiting.len() as u32, "meeting"),
            ));
            for j in waiting {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.add_css_class("item");
                row.append(&label(DOC, &["glyph", "busy-text"]));
                let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
                text.set_hexpand(true);
                text.append(&label(
                    &format!("Meeting \u{b7} {}", file_name(&j.dir)),
                    &["title"],
                ));
                let what = match (j.state, &j.behind) {
                    (JobState::Running, Some(b)) => format!("queued behind {b}"),
                    (JobState::Running, None) if j.total > 0 => {
                        format!("piece {} of {}", (j.done + 1).min(j.total), j.total)
                    }
                    (JobState::Running, None) => "starting".into(),
                    _ => "queued".into(),
                };
                text.append(&label(&what, &["dim", "small"]));
                row.append(&text);
                if j.state == JobState::Queued {
                    let b = button(CLOSE, "", &["small", "icon"]);
                    b.set_tooltip_text(Some("Take it off the queue"));
                    let dir = j.dir.clone();
                    self.bind(&b, move |_| {
                        send_quiet(Request::Forget { dir: dir.clone() })
                    });
                    row.append(&b);
                }
                self.transcripts.append(&row);
            }
        }
        if !failed.is_empty() {
            self.transcripts.append(&lab_row("Failed", ""));
            for j in failed {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                row.add_css_class("item");
                row.append(&label(DOC, &["glyph", "bad"]));
                let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
                text.set_hexpand(true);
                text.append(&label(
                    &format!("Meeting \u{b7} {}", file_name(&j.dir)),
                    &["title"],
                ));
                let e = label(j.error.as_deref().unwrap_or("failed"), &["dim", "small"]);
                e.set_wrap(true);
                e.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                text.append(&e);
                row.append(&text);
                let b = button(RETRY, "Retry", &["small"]);
                let dir = j.dir.clone();
                self.bind(&b, move |_| send_quiet(Request::Queue { dir: dir.clone() }));
                row.append(&b);
                let b = button(CLOSE, "", &["small", "icon"]);
                b.set_tooltip_text(Some("Dismiss"));
                let dir = j.dir.clone();
                self.bind(&b, move |_| {
                    send_quiet(Request::Forget { dir: dir.clone() })
                });
                row.append(&b);
                self.transcripts.append(&row);
            }
        }
        let ready: Vec<&Recording> = s
            .recordings
            .iter()
            .filter(|r| r.kind == Kind::Meeting)
            .filter(|r| matches!(r.transcript, Transcript::Ready | Transcript::Missing))
            .collect();
        if !ready.is_empty() {
            self.transcripts.append(&lab_row(
                "Meetings",
                &format!(
                    "{} ready",
                    ready
                        .iter()
                        .filter(|r| r.transcript == Transcript::Ready)
                        .count()
                ),
            ));
            for r in ready {
                self.transcripts.append(&self.row(r, s.now));
            }
        }
        if self.transcripts.first_child().is_none() {
            let c = card(&[]);
            let t = label(
                "No meetings yet. When a meeting stops, its transcript is queued and shows up here.",
                &[],
            );
            t.set_wrap(true);
            c.append(&t);
            self.transcripts.append(&c);
        }
    }
}
