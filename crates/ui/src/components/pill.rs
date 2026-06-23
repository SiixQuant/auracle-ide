use crate::prelude::*;
use gpui::{AnyView, Hsla, IntoElement, ParentElement, Styled};

/// The three Runway progress tones a [`Pill`] can take. Kept here in `ui` so any
/// crate can map its own state enum to a pill tone without depending on theme
/// internals; resolved to concrete `cx.theme()` colours at render time.
#[derive(Clone, Copy)]
pub enum PillTone {
    Accent,
    Positive,
    Muted,
}

/// Where a [`Pill`]'s colours come from. Two deliberately separate vocabularies:
/// a fixed agent pastel (a brand value plus its own ink), or a theme progress
/// tone resolved against `cx.theme()`.
#[derive(Clone)]
enum PillPaint {
    /// A fixed brand pastel fill with a fixed ink. The caller has already
    /// converted the brand RGB triples to `Hsla`.
    Pastel { fill: Hsla, ink: Hsla },
    /// A theme tone resolved at render time; the fill is a faint wash of the same
    /// colour used for the ink.
    Tone(PillTone),
}

/// A rounded-FULL caption pill: a small uppercase label inside a stadium of
/// either a fixed agent pastel or a theme progress tone. A radius-only sibling of
/// [`Chip`](crate::Chip) — same buffer-font label, custom fill/ink, pill radius.
#[derive(IntoElement)]
pub struct Pill {
    label: SharedString,
    paint: PillPaint,
    tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>>,
}

impl Pill {
    /// A pill painted with a fixed agent pastel. `fill` and `ink` are the already
    /// theme-converted brand colours; the label is rendered uppercase.
    pub fn pastel(label: impl Into<SharedString>, fill: Hsla, ink: Hsla) -> Self {
        Self {
            label: label.into(),
            paint: PillPaint::Pastel { fill, ink },
            tooltip: None,
        }
    }

    /// A pill painted with a theme progress tone (resolved at render).
    pub fn tone(label: impl Into<SharedString>, tone: PillTone) -> Self {
        Self {
            label: label.into(),
            paint: PillPaint::Tone(tone),
            tooltip: None,
        }
    }

    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }
}

impl RenderOnce for Pill {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (fill, ink) = match self.paint {
            PillPaint::Pastel { fill, ink } => (fill, ink),
            PillPaint::Tone(tone) => {
                let ink = match tone {
                    PillTone::Accent => cx.theme().colors().text_accent,
                    PillTone::Positive => cx.theme().status().success,
                    PillTone::Muted => cx.theme().colors().text_muted,
                };
                // Runway = progress, never a pastel: a faint wash of the tone's ink.
                (ink.opacity(0.12), ink)
            }
        };

        h_flex()
            .flex_none()
            .id(self.label.clone())
            .px_1p5()
            .rounded_full()
            .bg(fill)
            .overflow_hidden()
            .child(
                Label::new(self.label.to_uppercase())
                    .size(LabelSize::XSmall)
                    .color(Color::Custom(ink))
                    .buffer_font(cx),
            )
            .when_some(self.tooltip, |this, tooltip| this.tooltip(tooltip))
    }
}
