//! Native GPUI equity chart. A thin painter over `auracle_chart_layout` geometry
//! (scales + stride downsample) — no web libs. It renders an honest empty state
//! when there is no series (the engine does not return one yet), and never
//! synthesises data: it draws exactly the points it is given.

use auracle_chart_layout::{Range, Scale, downsample_stride, nice_ticks};
use gpui::{IntoElement, PathBuilder, canvas, point};
use ui::prelude::*;

/// A line + area equity chart. Construct with `(t, equity)` points; finite-only
/// points are drawn. Fewer than two finite points renders the empty state.
#[derive(IntoElement)]
pub struct EquityChart {
    points: Vec<(f64, f64)>,
}

impl EquityChart {
    pub fn new(points: Vec<(f64, f64)>) -> Self {
        Self { points }
    }
}

impl RenderOnce for EquityChart {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let points: Vec<(f64, f64)> = self
            .points
            .into_iter()
            .filter(|(t, v)| t.is_finite() && v.is_finite())
            .collect();

        if points.len() < 2 {
            return v_flex()
                .h(px(160.))
                .w_full()
                .items_center()
                .justify_center()
                .gap_1()
                .border_1()
                .border_color(cx.theme().colors().border_variant)
                .rounded_md()
                .child(
                    Label::new("No equity curve yet")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Label::new("The engine has not returned an equity series for this run.")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
                .into_any_element();
        }

        let line_color = cx.theme().colors().text_accent;
        let area_color = line_color.opacity(0.10);
        let grid_color = cx.theme().colors().border_variant;
        let height = px(160.);

        let first = points.first().map(|p| p.1).unwrap_or(0.0);
        let last = points.last().map(|p| p.1).unwrap_or(0.0);
        let caption = format!("{first:.2} → {last:.2}  ·  {} points", points.len());

        let chart = canvas(
            |_, _, _| {},
            move |bounds, _, window, _cx| {
                let w = f32::from(bounds.size.width).max(1.0) as f64;
                let h = f32::from(bounds.size.height).max(1.0) as f64;
                let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
                let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
                let x_scale =
                    Scale::new(Range::from_values(&xs).unwrap_or(Range::new(0.0, 1.0)), w);
                let y_range = Range::from_values(&ys).unwrap_or(Range::new(0.0, 1.0));
                let y_scale = Scale::new(y_range, h);
                let keep = downsample_stride(points.len(), w.max(2.0) as usize);

                let origin_x = bounds.origin.x;
                let origin_y = bounds.origin.y;
                let baseline = origin_y + px(h as f32);

                // Faint editorial gridlines at "nice" equity levels, behind the
                // curve — gives a sense of scale without axis clutter.
                for tick in nice_ticks(y_range, 4) {
                    if tick < y_range.min || tick > y_range.max {
                        continue;
                    }
                    let y = origin_y + px((h - y_scale.to_pixel(tick)) as f32);
                    let mut gridline = PathBuilder::stroke(px(1.));
                    gridline.move_to(point(origin_x, y));
                    gridline.line_to(point(origin_x + px(w as f32), y));
                    if let Ok(path) = gridline.build() {
                        window.paint_path(path, grid_color);
                    }
                }
                // Equity rises upward, so invert the y pixel (screen y grows down).
                let screen = |i: usize| {
                    let (t, v) = points[i];
                    point(
                        origin_x + px(x_scale.to_pixel(t) as f32),
                        origin_y + px((h - y_scale.to_pixel(v)) as f32),
                    )
                };

                if let Some(&first_idx) = keep.first() {
                    let head = screen(first_idx);

                    let mut area = PathBuilder::fill();
                    area.move_to(point(head.x, baseline));
                    area.line_to(head);
                    for &i in keep.iter().skip(1) {
                        area.line_to(screen(i));
                    }
                    if let Some(&tail_idx) = keep.last() {
                        area.line_to(point(screen(tail_idx).x, baseline));
                    }
                    if let Ok(path) = area.build() {
                        window.paint_path(path, area_color);
                    }

                    let mut line = PathBuilder::stroke(px(1.5));
                    line.move_to(head);
                    for &i in keep.iter().skip(1) {
                        line.line_to(screen(i));
                    }
                    if let Ok(path) = line.build() {
                        window.paint_path(path, line_color);
                    }
                }
            },
        )
        .h(height)
        .w_full();

        v_flex()
            .w_full()
            .gap_1()
            .child(chart)
            .child(
                Label::new(caption)
                    .size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .buffer_font(cx),
            )
            .into_any_element()
    }
}
