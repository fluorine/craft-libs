//! Which of the application's windows lies under a point on the desktop (feature
//! `native-windows`). Points and rects are in desktop logical points.

use egui::{Pos2, Rect};

/// Where a desktop point falls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Over {
    /// The floating native window with this [`super::Floating::id`], at this point in its own
    /// coordinates.
    Window(u64, Pos2),
    /// The main window, at this point in its coordinates.
    Main(Pos2),
    /// None of the application's windows.
    Desktop,
}

/// The window under `point`: the first of `windows` (floating native windows by id and desktop
/// rect, front first) that holds it, then `main` (the main window's content). The window
/// `dragged` follows the pointer by its bar and is never its own target.
pub(super) fn window_at(point: Pos2, windows: &[(u64, Rect)], main: Option<Rect>, dragged: Option<u64>) -> Over {
    if !point.is_finite() {
        return Over::Desktop;
    }
    if let Some((id, rect)) = windows.iter().find(|(id, rect)| Some(*id) != dragged && rect.contains(point)) {
        return Over::Window(*id, point - rect.min.to_vec2());
    }
    match main {
        Some(main) if main.contains(point) => Over::Main(point - main.min.to_vec2()),
        _ => Over::Desktop,
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn the_front_window_wins_then_the_main_window_then_the_desktop() {
        let main = Some(rect(100.0, 50.0, 800.0, 600.0));
        // Window 2 lies over window 1, and both over the main window.
        let windows = [(2, rect(300.0, 100.0, 200.0, 200.0)), (1, rect(250.0, 80.0, 200.0, 200.0))];
        assert_eq!(window_at(pos2(350.0, 150.0), &windows, main, None), Over::Window(2, pos2(50.0, 50.0)));
        assert_eq!(window_at(pos2(260.0, 90.0), &windows, main, None), Over::Window(1, pos2(10.0, 10.0)));
        assert_eq!(window_at(pos2(120.0, 60.0), &windows, main, None), Over::Main(pos2(20.0, 10.0)));
        assert_eq!(window_at(pos2(20.0, 20.0), &windows, main, None), Over::Desktop);
        // A floating window outside the main window.
        let windows = [(3, rect(1000.0, 0.0, 100.0, 100.0))];
        assert_eq!(window_at(pos2(1050.0, 40.0), &windows, main, None), Over::Window(3, pos2(50.0, 40.0)));
        // Unknown main window, a point that is not one.
        assert_eq!(window_at(pos2(120.0, 60.0), &[], None, None), Over::Desktop);
        assert_eq!(window_at(pos2(f32::NAN, 60.0), &windows, main, None), Over::Desktop);
    }

    #[test]
    fn the_dragged_window_is_looked_through() {
        let main = Some(rect(0.0, 0.0, 800.0, 600.0));
        let windows = [(2, rect(300.0, 100.0, 200.0, 200.0)), (1, rect(250.0, 80.0, 200.0, 200.0))];
        assert_eq!(window_at(pos2(350.0, 150.0), &windows, main, Some(2)), Over::Window(1, pos2(100.0, 70.0)));
        assert_eq!(window_at(pos2(350.0, 150.0), &windows, main, Some(1)), Over::Window(2, pos2(50.0, 50.0)));
        assert_eq!(window_at(pos2(480.0, 280.0), &windows, main, Some(2)), Over::Main(pos2(480.0, 280.0)));
    }
}
