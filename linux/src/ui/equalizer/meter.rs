//! A thin vertical level bar for the preamp rail.

use super::render::{self, Rgb};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub struct MeterBar {
    pub area: gtk::DrawingArea,
    fraction: Cell<f64>,
    color: Cell<Rgb>,
}

impl MeterBar {
    pub fn new() -> Rc<Self> {
        let area = gtk::DrawingArea::builder()
            .content_width(7)
            .vexpand(true)
            .hexpand(false)
            .build();
        area.update_property(&[gtk::accessible::Property::Label("Output level")]);
        let bar = Rc::new(Self {
            area,
            fraction: Cell::new(0.0),
            color: Cell::new(render::LOSSLESS),
        });
        let weak = Rc::downgrade(&bar);
        bar.area.set_draw_func(move |_, cr, width, height| {
            let Some(bar) = weak.upgrade() else { return };
            let (w, h) = (width as f64, height as f64);
            let ink = if adw::StyleManager::default().is_dark() { 1.0 } else { 0.0 };
            cr.set_source_rgba(ink, ink, ink, 0.1);
            rounded(cr, 0.0, 0.0, w, h, 3.0);
            let _ = cr.fill();
            let fill = h * bar.fraction.get();
            if fill > 0.5 {
                let (r, g, b) = bar.color.get();
                cr.set_source_rgba(r, g, b, 1.0);
                rounded(cr, 0.0, h - fill, w, fill, 3.0);
                let _ = cr.fill();
            }
        });
        bar
    }

    pub fn set(&self, peak_decibels: f32, color: Rgb) {
        let fraction = render::meter_fraction(peak_decibels);
        if (fraction - self.fraction.get()).abs() > 0.002 || color != self.color.get() {
            self.fraction.set(fraction);
            self.color.set(color);
            self.area.queue_draw();
        }
    }
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + radius, y + height - radius, radius, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + radius, y + radius, radius, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}
