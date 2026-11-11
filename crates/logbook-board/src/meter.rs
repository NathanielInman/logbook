//! The level meter on the live card: the loudness of the last moments of
//! the chunk being written, as a row of bars that scroll left. The color is
//! the widget's CSS `color`, so the theme decides it.

use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

const BARS: usize = 18;

#[derive(Clone)]
pub struct Meter {
    pub area: gtk::DrawingArea,
    levels: Rc<RefCell<VecDeque<f64>>>,
}

impl Meter {
    pub fn new() -> Meter {
        let area = gtk::DrawingArea::new();
        area.add_css_class("meter");
        area.set_content_height(26);
        area.set_content_width(BARS as i32 * 6);
        area.set_valign(gtk::Align::Center);
        let levels = Rc::new(RefCell::new(VecDeque::from(vec![0.0; BARS])));
        let l = levels.clone();
        area.set_draw_func(move |a, cr, w, h| {
            let c = a.color();
            cr.set_source_rgba(
                c.red() as f64,
                c.green() as f64,
                c.blue() as f64,
                c.alpha() as f64,
            );
            let levels: std::cell::Ref<VecDeque<f64>> = l.borrow();
            let n = levels.len().max(1) as f64;
            let slot = w as f64 / n;
            let bar = (slot * 0.6).max(2.0);
            for (i, v) in levels.iter().enumerate() {
                let bh = (h as f64 * (0.12 + 0.88 * v.clamp(0.0_f64, 1.0))).max(2.0);
                let x = i as f64 * slot + (slot - bar) / 2.0;
                let y = (h as f64 - bh) / 2.0;
                rounded(cr, x, y, bar, bh, bar / 2.0);
                let _ = cr.fill();
            }
        });
        Meter { area, levels }
    }

    pub fn push(&self, v: f64) {
        let mut l = self.levels.borrow_mut();
        l.pop_front();
        l.push_back(v);
        drop(l);
        self.area.queue_draw();
    }

    pub fn clear(&self) {
        let mut l = self.levels.borrow_mut();
        l.iter_mut().for_each(|x| *x = 0.0);
        drop(l);
        self.area.queue_draw();
    }
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -pi / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, pi / 2.0);
    cr.arc(x + r, y + h - r, r, pi / 2.0, pi);
    cr.arc(x + r, y + r, r, pi, 1.5 * pi);
    cr.close_path();
}
