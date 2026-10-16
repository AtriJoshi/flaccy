use crate::filters::LibraryFilter;
use crate::recap::{Period, ALL_PERIODS};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// The row of quick-pivot chips over the Songs and Albums pages: one is always
/// selected. When the set includes Most Played its chip names the time range it
/// counts over and carries a small menu to change it.
pub struct ChipBar {
    pub widget: gtk::Widget,
    chips: Vec<(LibraryFilter, gtk::ToggleButton)>,
    range: Cell<Period>,
    range_checks: Vec<(Period, gtk::Image)>,
    silent: Cell<bool>,
}

impl ChipBar {
    pub fn new(
        filters: &[LibraryFilter],
        selected: LibraryFilter,
        range: Period,
        on_filter: impl Fn(LibraryFilter) + 'static,
        on_range: impl Fn(Period) + 'static,
    ) -> Rc<ChipBar> {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(10)
            .margin_bottom(4)
            .margin_start(12)
            .margin_end(12)
            .build();

        let on_filter: Rc<dyn Fn(LibraryFilter)> = Rc::new(on_filter);
        let on_range: Rc<dyn Fn(Period)> = Rc::new(on_range);
        let mut chips: Vec<(LibraryFilter, gtk::ToggleButton)> = Vec::new();
        let mut range_checks: Vec<(Period, gtk::Image)> = Vec::new();
        let mut range_menu: Option<gtk::MenuButton> = None;

        for filter in filters {
            let chip = gtk::ToggleButton::builder().label(filter.label()).build();
            chip.add_css_class("pill");
            chip.add_css_class("chip");
            if let Some((_, leader)) = chips.first() {
                chip.set_group(Some(leader));
            }
            chip.set_active(*filter == selected);
            chips.push((*filter, chip));
        }

        if filters.contains(&LibraryFilter::MostPlayed) {
            let (menu, checks) = range_picker(range);
            range_menu = Some(menu);
            range_checks = checks;
        }

        for (filter, chip) in &chips {
            match (filter, &range_menu) {
                (LibraryFilter::MostPlayed, Some(menu)) => {
                    let pair = gtk::Box::builder()
                        .orientation(gtk::Orientation::Horizontal)
                        .spacing(2)
                        .build();
                    pair.append(chip);
                    pair.append(menu);
                    row.append(&pair);
                }
                _ => row.append(chip),
            }
        }

        let bar = Rc::new(ChipBar {
            widget: row.clone().upcast(),
            chips,
            range: Cell::new(range),
            range_checks,
            silent: Cell::new(false),
        });
        bar.refresh_range_label();

        for (filter, chip) in &bar.chips {
            let filter = *filter;
            let bar_ref = Rc::clone(&bar);
            let on_filter = Rc::clone(&on_filter);
            chip.connect_toggled(move |chip| {
                if bar_ref.silent.get() || !chip.is_active() {
                    return;
                }
                on_filter(filter);
            });
        }

        if let Some(menu) = range_menu {
            wire_range_choices(&bar, &menu, on_range);
        }
        bar
    }

    pub fn set_selected(&self, filter: LibraryFilter) {
        self.silent.set(true);
        for (candidate, chip) in &self.chips {
            if *candidate == filter {
                chip.set_active(true);
            }
        }
        self.silent.set(false);
    }

    pub fn set_range(&self, range: Period) {
        self.range.set(range);
        self.refresh_range_label();
    }

    fn refresh_range_label(&self) {
        let range = self.range.get();
        for (filter, chip) in &self.chips {
            if *filter == LibraryFilter::MostPlayed {
                chip.set_label(&format!("{} · {}", filter.label(), range.label()));
            }
        }
        for (period, check) in &self.range_checks {
            check.set_opacity(if *period == range { 1.0 } else { 0.0 });
        }
    }
}

/// The small arrow beside Most Played and the popover it opens. A plain
/// popover, not a menu model: GTK 4.22's `PopoverMenu` clips trailing rows.
fn range_picker(current: Period) -> (gtk::MenuButton, Vec<(Period, gtk::Image)>) {
    let menu = gtk::MenuButton::builder()
        .icon_name("pan-down-symbolic")
        .tooltip_text("Choose the time range Most Played counts over")
        .valign(gtk::Align::Center)
        .build();
    menu.add_css_class("flat");
    menu.add_css_class("circular");

    let list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let mut checks = Vec::new();
    for period in ALL_PERIODS {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();
        let name = gtk::Label::builder()
            .label(period.display_name())
            .xalign(0.0)
            .hexpand(true)
            .build();
        let check = gtk::Image::from_icon_name("object-select-symbolic");
        check.set_opacity(if period == current { 1.0 } else { 0.0 });
        content.append(&name);
        content.append(&check);
        button.set_child(Some(&content));
        button.set_widget_name(period.api_value());
        list.append(&button);
        checks.push((period, check));
    }
    let popover = gtk::Popover::new();
    popover.set_child(Some(&list));
    menu.set_popover(Some(&popover));
    (menu, checks)
}

fn wire_range_choices(bar: &Rc<ChipBar>, menu: &gtk::MenuButton, on_range: Rc<dyn Fn(Period)>) {
    let Some(popover) = menu.popover() else {
        return;
    };
    let Some(list) = popover.child().and_downcast::<gtk::Box>() else {
        return;
    };
    let mut child = list.first_child();
    while let Some(button) = child.clone().and_downcast::<gtk::Button>() {
        child = button.next_sibling();
        let Some(period) = ALL_PERIODS
            .into_iter()
            .find(|period| period.api_value() == button.widget_name())
        else {
            continue;
        };
        let bar = Rc::clone(bar);
        let popover = popover.clone();
        let on_range = Rc::clone(&on_range);
        button.connect_clicked(move |_| {
            popover.popdown();
            bar.set_range(period);
            bar.set_selected(LibraryFilter::MostPlayed);
            on_range(period);
        });
    }
}
