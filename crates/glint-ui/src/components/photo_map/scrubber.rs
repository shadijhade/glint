//! The date scrubber along the bottom of the map: the years the photos were taken in, each a small bar as tall as
//! its count with the year under it, and the months of the chosen year. Pressing one shows only that time (the
//! camera flies to what is left); pressing it again goes back out. The "Arcs" switch at its end draws the trip.

use super::*;
use glint_model::world_map::{month_range, year_range, Timeline};

/// What time the map shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Scope {
    /// Every photo.
    #[default]
    All,
    Year(i32),
    /// A year and one of its months (1 to 12).
    Month(i32, u32),
}

impl Scope {
    /// The times (ms, both ends included) this scope shows; None for all of them.
    pub(super) fn range(self) -> Option<(i64, i64)> {
        match self {
            Scope::All => None,
            Scope::Year(year) => Some(year_range(year)),
            Scope::Month(year, month) => Some(month_range(year, month)),
        }
    }

    /// The year this scope is in, if it is in one.
    pub(super) fn year(self) -> Option<i32> {
        match self {
            Scope::All => None,
            Scope::Year(year) | Scope::Month(year, _) => Some(year),
        }
    }

    /// Where pressing a year leads: into it, or (when it is the one shown) back out.
    pub(super) fn press_year(self, year: i32) -> Scope {
        if self == Scope::Year(year) {
            Scope::All
        } else {
            Scope::Year(year)
        }
    }

    /// Where pressing a month of `year` leads: into it, or (when it is the one shown) back up to its year.
    pub(super) fn press_month(self, year: i32, month: u32) -> Scope {
        if self == Scope::Month(year, month) {
            Scope::Year(year)
        } else {
            Scope::Month(year, month)
        }
    }
}

/// A month's short name.
fn month_name(month: u32) -> SharedString {
    match month {
        1 => t!("Jan"),
        2 => t!("Feb"),
        3 => t!("Mar"),
        4 => t!("Apr"),
        5 => t!("May"),
        6 => t!("Jun"),
        7 => t!("Jul"),
        8 => t!("Aug"),
        9 => t!("Sep"),
        10 => t!("Oct"),
        11 => t!("Nov"),
        _ => t!("Dec"),
    }
}

/// The tallest a bar is, and the least, in CSS pixels.
const BAR_MAX: f32 = 22.0;
const BAR_MIN: f32 = 2.0;

impl PhotoMapView {
    /// Shows only `scope`'s time: what is left is fitted by a flight of the camera.
    pub(super) fn set_scope(&mut self, scope: Scope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.touched = true;
            self.selected_cluster = None;
            cx.notify();
        }
    }

    /// The scrubber, or nothing while there are no photos.
    pub(super) fn scrubber(&mut self, timeline: &Timeline, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if timeline.years.is_empty() {
            return None;
        }
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        let scope = self.scope;
        let most = timeline.most().max(1) as f32;
        let bar = |count: u32, on: bool, widest: f32| {
            div()
                .w(px(widest))
                .h(px(BAR_MIN + (BAR_MAX - BAR_MIN) * count as f32 / most))
                .rounded_t(px(2.))
                .bg(if on {
                    t.primary
                } else {
                    t.muted_foreground.opacity(0.45)
                })
        };

        let years: Vec<Div> = timeline
            .years
            .iter()
            .map(|bucket| {
                let year = bucket.year;
                let on = scope.year() == Some(year);
                col()
                    .items_center()
                    .gap(px(2.))
                    .flex_none()
                    .child(bar(bucket.count, on, 16.0))
                    .child(
                        Button::new(("pm-year", year.max(0) as usize))
                            .compact()
                            .ghost()
                            .label(year.to_string())
                            .toggled(scope == Scope::Year(year))
                            .accessibility_label(t!("{year}, {n} photos", year = year.to_string(), n = bucket.count))
                            .tooltip(t!("{n} photos in {year}", n = bucket.count, year = year.to_string()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = this.scope.press_year(year);
                                this.set_scope(next, cx);
                            })),
                    )
            })
            .collect();

        let months = scope.year().and_then(|year| timeline.year(year)).map(|bucket| {
            let year = bucket.year;
            let mut cells: Vec<Div> = Vec::with_capacity(12);
            for month in 1..=12u32 {
                cells.push({
                    let count = bucket.months[month as usize - 1];
                    let name = month_name(month);
                    col()
                        .items_center()
                        .gap(px(2.))
                        .flex_none()
                        .child(bar(count, scope == Scope::Month(year, month), 12.0))
                        .child(
                            Button::new(("pm-month", month as usize))
                                .compact()
                                .ghost()
                                .label(name.clone())
                                .toggled(scope == Scope::Month(year, month))
                                .disabled(count == 0)
                                .accessibility_label(t!(
                                    "{month} {year}, {n} photos",
                                    month = name.clone(),
                                    year = year.to_string(),
                                    n = count
                                ))
                                .tooltip(t!(
                                    "{n} photos in {month} {year}",
                                    n = count,
                                    month = name,
                                    year = year.to_string()
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let next = this.scope.press_month(year, month);
                                    this.set_scope(next, cx);
                                })),
                        )
                });
            }
            row_start().items_end().gap(space(1.)).children(cells)
        });

        Some(
            col()
                .id(("pm-scrubber", pane))
                .role(Role::Group)
                .aria_label(t!("Time range"))
                .w_full()
                .flex_none()
                .bg(t.background)
                .border_t_1()
                .border_color(t.border)
                .px(space(2.))
                .py(space(1.))
                .gap(space(1.))
                .child(
                    row_start()
                        .items_end()
                        .gap(space(2.))
                        .child(
                            Button::new(("pm-all-time", pane))
                                .compact()
                                .ghost()
                                .label(t!("All time"))
                                .toggled(scope == Scope::All)
                                .on_click(cx.listener(|this, _, _, cx| this.set_scope(Scope::All, cx))),
                        )
                        .child(
                            div()
                                .id(("pm-years", pane))
                                .flex_1()
                                .min_w_0()
                                .overflow_x_scroll()
                                .child(row_start().items_end().gap(space(1.)).children(years)),
                        )
                        .child(
                            Button::new(("pm-arcs", pane))
                                .compact()
                                .ghost()
                                .label(t!("Arcs"))
                                .toggled(self.arcs)
                                .tooltip(t!("Draw dashed arcs between the places in the order they were visited"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.arcs = !this.arcs;
                                    cx.notify();
                                })),
                        ),
                )
                .when_some(months, |scrubber, months| scrubber.child(months)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressing_a_year_goes_in_and_pressing_it_again_goes_out() {
        let mut scope = Scope::All;
        scope = scope.press_year(2019);
        assert_eq!(scope, Scope::Year(2019));
        scope = scope.press_year(2020);
        assert_eq!(scope, Scope::Year(2020));
        scope = scope.press_year(2020);
        assert_eq!(scope, Scope::All);
        // A month goes in from its year, out to its year, and a year goes back to the year from a month.
        scope = Scope::Year(2019).press_month(2019, 7);
        assert_eq!(scope, Scope::Month(2019, 7));
        assert_eq!(scope.press_month(2019, 7), Scope::Year(2019));
        assert_eq!(scope.press_month(2019, 8), Scope::Month(2019, 8));
        assert_eq!(scope.press_year(2019), Scope::Year(2019));
        assert_eq!(scope.year(), Some(2019));
        assert_eq!(Scope::All.year(), None);
    }

    #[test]
    fn a_scope_shows_the_times_of_its_year_or_month() {
        assert_eq!(Scope::All.range(), None);
        let (from, to) = Scope::Year(2024).range().expect("a range");
        assert!(from < to);
        let (jan_from, jan_to) = Scope::Month(2024, 1).range().expect("a range");
        assert_eq!(jan_from, from);
        let (dec_from, dec_to) = Scope::Month(2024, 12).range().expect("a range");
        assert_eq!(dec_to, to);
        assert!(jan_to < dec_from);
    }
}
