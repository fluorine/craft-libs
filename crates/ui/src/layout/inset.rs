//! Decoration-free indentation for dialog fields and inspector subgroups.

use egui::{InnerResponse, Ui, UiBuilder};

/// Physical left/right padding around a scope, in logical points.
///
/// Unlike a tree indent, this has no guide line and does not require a vertical parent. It
/// supports fractional theme spacing without changing global `Spacing::indent` or visuals.
/// The child inherits layout, enabled state and clipping; choose an inner layout as usual when
/// placing a vertical set of fields inside a horizontal parent. Translation, RTL padding policy,
/// widget identity and whether controls are enabled remain the application's responsibility.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContentInset {
    left: f32,
    right: f32,
}

impl ContentInset {
    /// Indent from the left. Negative/non-finite values become zero; finite values are capped
    /// at one million points and further restricted to the available width when shown.
    pub fn new(left: f32) -> Self {
        Self { left: amount(left), right: 0.0 }
    }

    pub fn right(mut self, right: f32) -> Self {
        self.right = amount(right);
        self
    }

    /// Allocate the child's used space and return its value and response. The left inset takes
    /// priority in narrow parents; the right inset uses the remaining width. Insets introduce
    /// no vertical gap or frame. Repeated controls can use an enclosing `Ui::push_id`.
    pub fn show<R>(self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
        let mut rect = ui.available_rect_before_wrap();
        let left = self.left.min(rect.width().max(0.0));
        rect.min.x += left;
        rect.max.x -= self.right.min(rect.width().max(0.0));
        ui.scope_builder(UiBuilder::new().max_rect(rect), add_contents)
    }
}

fn amount(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1_000_000.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, RawInput, Rect, Vec2};

    #[test]
    fn fractional_padding_keeps_fields_below_heading_and_following_rows_below_contents() {
        let ctx = Context::default();
        ctx.run_ui(RawInput::default(), |ui| {
            let heading = ui.label("Settings");
            let original_indent = ui.spacing().indent;
            let fields = ContentInset::new(12.5).right(4.25).show(ui, |ui| {
                assert!((ui.max_rect().left() - heading.rect.left() - 12.5).abs() < 0.01);
                ui.label("First");
                ui.label("Second");
                7
            });
            assert_eq!(fields.inner, 7);
            let footer = ui.label("Footer");
            assert!(footer.rect.top() >= fields.response.rect.bottom());
            assert_eq!(ui.spacing().indent, original_indent);
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn narrow_and_horizontal_parents_and_invalid_insets_do_not_invert_child_rectangles() {
        let ctx = Context::default();
        for value in [f32::NAN, f32::INFINITY, -1.0, 0.0, 12.5, f32::MAX] {
            ctx.run_ui(RawInput::default(), |ui| {
                ui.horizontal(|ui| {
                    ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min, Vec2::new(5.0, 50.0))), |ui| {
                        ContentInset::new(value).right(value).show(ui, |ui| {
                            assert!(ui.max_rect().is_finite());
                            assert!(ui.max_rect().width() >= 0.0);
                        });
                    });
                });
            })
            .drop_without_applying_deltas();
        }
    }
}
