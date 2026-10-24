//! [D] Docks: side docks of columns, each a column bar above stacked groups.
//!
//! Columns run from the window edge inwards. Every column's inner edge resizes it; the last
//! column's inner edge is the dock's inner edge. Behavior follows VectorCraft `floating.rs`
//! (column layout, drop geometry) and PhotoCraft `dock.rs` (group heights).

use std::{fmt::Debug, hash::Hash};

use egui::{Context, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};

use super::{GroupKind, Renderer, RowItem, row_drag, row_range};
use crate::docking::{
    Action, Column, DockEvent, DockStyle, Group, GroupDrag, GroupGesture, Node, Placement, Side, Target,
    dock_model::Unit,
    targets::{DockDrop, DockTarget, column_edge, horizontal_line, resolve_dock, usable, vertical_line},
};
use crate::layout::{ResizeHandle, ResizeHandleStyle, SplitAxis};

/// Width of the »/« button at the outer end of a column bar.
const COLLAPSE_WIDTH: f32 = 18.0;

/// The physical window edge a dock sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::docking) enum Edge {
    Left,
    Right,
}

/// `Start` is the left edge in a left-to-right layout and the right edge in a right-to-left one.
pub(in crate::docking) fn edge_of(side: Side, right_to_left: bool) -> Edge {
    match (side, right_to_left) {
        (Side::Start, false) | (Side::End, true) => Edge::Left,
        (Side::Start, true) | (Side::End, false) => Edge::Right,
    }
}

fn side_at(edge: Edge, right_to_left: bool) -> Side {
    match (edge, right_to_left) {
        (Edge::Left, false) | (Edge::Right, true) => Side::Start,
        (Edge::Left, true) | (Edge::Right, false) => Side::End,
    }
}

/// The rects of a dock beside `other` in `rect`, and the widest the dock may get.
pub(in crate::docking) fn dock_rects<P>(
    side: Side,
    columns: &[Column<P>],
    other: &Node<P>,
    rect: Rect,
    style: &DockStyle,
    content: &dyn crate::docking::DockContent<P>,
) -> (Rect, Rect, f32) {
    let gap = style.gap;
    let room = (rect.width() - gap - super::node_limits(other, style, content).min.x).clamp(0.0, rect.width());
    let width = columns.iter().map(|c| column_width(c, style)).sum::<f32>().clamp(0.0, room);
    let (dock, rest) = match edge_of(side, style.right_to_left) {
        Edge::Left => (
            Rect::from_min_max(rect.min, pos2(rect.left() + width, rect.bottom())),
            Rect::from_min_max(pos2((rect.left() + width + gap).min(rect.right()), rect.top()), rect.max),
        ),
        Edge::Right => (
            Rect::from_min_max(pos2(rect.right() - width, rect.top()), rect.max),
            Rect::from_min_max(rect.min, pos2((rect.right() - width - gap).max(rect.left()), rect.bottom())),
        ),
    };
    (dock, rest, room)
}

pub(in crate::docking) fn column_width<P>(column: &Column<P>, style: &DockStyle) -> f32 {
    if column.iconic { column.iconic_width.unwrap_or(style.iconic_width).clamp(style.iconic_width, style.iconic_max_width) } else { column.width }
}

/// Heights of stacked groups in a column body `room` high. Each entry is (minimized, saved
/// height, minimum). Minimized groups take `strip`, groups with a saved height keep it, and the
/// rest share what is left; when every open group has a height, the last one takes the rest.
/// Saved heights shrink towards their minimums before the column overflows and scrolls.
pub(in crate::docking) fn group_heights(groups: &[(bool, Option<f32>, f32)], strip: f32, room: f32) -> Vec<f32> {
    let open: Vec<usize> = (0..groups.len()).filter(|i| groups.get(*i).is_some_and(|g| !g.0)).collect();
    let mut shared: Vec<usize> = open.iter().copied().filter(|i| groups.get(*i).is_some_and(|g| g.1.is_none_or(|h| !h.is_finite()))).collect();
    if shared.is_empty() {
        shared.extend(open.last());
    }
    let mut heights: Vec<f32> = groups
        .iter()
        .enumerate()
        .map(|(i, (minimized, height, min))| {
            if *minimized {
                strip
            } else if shared.contains(&i) {
                0.0
            } else {
                height.unwrap_or(*min).max(*min)
            }
        })
        .collect();
    let mins: Vec<f32> = groups.iter().map(|g| if g.0 { strip } else { g.2 }).collect();
    let shared_min: f32 = shared.iter().filter_map(|i| mins.get(*i)).sum();
    let deficit = heights.iter().sum::<f32>() + shared_min - room;
    let slack: f32 = open.iter().filter(|i| !shared.contains(i)).filter_map(|i| Some(heights.get(*i)? - mins.get(*i)?)).sum();
    if deficit > 0.0 && slack > 0.0 {
        let take = deficit.min(slack) / slack;
        for i in open.iter().filter(|i| !shared.contains(i)) {
            if let (Some(height), Some(min)) = (heights.get_mut(*i), mins.get(*i)) {
                *height -= (*height - min) * take;
            }
        }
    }
    let free = (room - heights.iter().sum::<f32>()).max(0.0);
    let share = free / shared.len().max(1) as f32;
    for i in &shared {
        if let (Some(height), Some(min)) = (heights.get_mut(*i), mins.get(*i)) {
            *height = share.max(*min);
        }
    }
    heights
}

/// Column widths that fit `room`. Each entry is (width, floor); a column with a floor shrinks
/// in proportion, never below its floor, and one without keeps its width. A dock saved on a
/// larger screen still fits, as far as the floors allow.
pub(in crate::docking) fn fit_widths(columns: &[(f32, Option<f32>)], room: f32) -> Vec<f32> {
    let mut widths: Vec<f32> = columns.iter().map(|(w, _)| *w).collect();
    for _ in 0..columns.len() {
        let total: f32 = widths.iter().sum();
        if total <= room {
            break;
        }
        let flex: f32 = widths.iter().zip(columns).filter(|(w, (_, floor))| floor.is_some_and(|f| **w > f)).map(|(w, _)| *w).sum();
        if flex <= 0.0 {
            break;
        }
        let scale = ((room - (total - flex)) / flex).max(0.0);
        for (width, (_, floor)) in widths.iter_mut().zip(columns) {
            if let Some(floor) = *floor
                && *width > floor
            {
                *width = (*width * scale).max(floor);
            }
        }
    }
    widths
}

impl<P: Clone + Eq + Hash + Debug + Send + Sync + 'static> Renderer<'_, P> {
    /// A horizontal split with a dock child: the dock takes its columns' width on its physical
    /// edge, the sibling the rest. False when neither child is a dock.
    pub(super) fn dock_split(
        &mut self,
        ui: &mut Ui,
        first: &Node<P>,
        second: &Node<P>,
        rect: Rect,
        path: &mut Vec<bool>,
        root: (&Node<P>, Rect),
    ) -> bool {
        let (dock_first, side, columns, other) = match (first, second) {
            (Node::Dock { side, columns }, other) => (true, *side, columns, other),
            (other, Node::Dock { side, columns }) => (false, *side, columns, other),
            _ => return false,
        };
        let (dock, rest, room) = dock_rects(side, columns, other, rect, self.style, &*self.content);
        self.dock(ui, side, columns, dock, room);
        path.push(dock_first);
        self.node(ui, other, rest, path, root);
        path.pop();
        true
    }

    /// One dock in `rect`, columns from its outer edge. `room` is the widest the dock may get.
    pub(super) fn dock(&mut self, ui: &mut Ui, side: Side, columns: &[Column<P>], rect: Rect, room: f32) {
        let edge = edge_of(side, self.style.right_to_left);
        let (outer, inward) = match edge {
            Edge::Left => (rect.left(), 1.0),
            Edge::Right => (rect.right(), -1.0),
        };
        if columns.is_empty() {
            let (band, line) = column_edge(outer, inward, rect.y_range());
            let band = band.union(Rect::from_x_y_ranges(
                outer.min(outer + inward * self.style.new_dock_band)..=outer.max(outer + inward * self.style.new_dock_band),
                rect.y_range(),
            ));
            self.dock_targets.push(DockDrop { target: DockTarget::NewDock { side }, band, line });
            return;
        }
        if usable(rect) {
            ui.painter().rect_filled(rect, 0.0, self.style.background);
        }
        let bounds: Vec<(f32, f32)> =
            columns.iter().map(|c| if c.iconic { (self.style.iconic_width, self.style.iconic_max_width) } else { self.column_bounds(c) }).collect();
        let natural: Vec<(f32, Option<f32>)> =
            columns.iter().zip(&bounds).map(|(c, (lo, _))| (column_width(c, self.style), (!c.iconic).then_some(lo.max(1.0)))).collect();
        let widths = fit_widths(&natural, rect.width());
        let scaled = widths.iter().zip(&natural).any(|(w, (n, _))| w != n);
        let total: f32 = widths.iter().sum();
        let mut x = outer;
        let mut drawn = Vec::new();
        for (index, column) in columns.iter().enumerate() {
            let Some(first) = column.groups.first().and_then(|g| g.panels.first()) else { continue };
            let fitted = widths.get(index).copied().unwrap_or_default();
            let width = fitted.min((rect.width() - (x - outer).abs()).max(0.0));
            let inner = x + inward * width;
            let column_rect = Rect::from_x_y_ranges(x.min(inner)..=x.max(inner), rect.y_range());
            self.column(ui, column, first, column_rect, edge);
            if index == 0 {
                let (band, line) = column_edge(x, inward, rect.y_range());
                self.dock_targets.push(DockDrop { target: DockTarget::ColumnEdge { anchor: first.clone(), after: false }, band, line });
            }
            let (band, line) = column_edge(inner, -inward, rect.y_range());
            self.dock_targets.push(DockDrop { target: DockTarget::ColumnEdge { anchor: first.clone(), after: true }, band, line });
            drawn.push((index, column, first, width, inner));
            x = inner;
        }
        // Registered after the columns, so the narrow hit areas win at the edges.
        for (index, column, first, width, inner) in drawn {
            if width < 1.0 {
                continue;
            }
            let others = total - widths.get(index).copied().unwrap_or_default();
            let (lo, hi) = bounds.get(index).copied().unwrap_or_default();
            let hi = hi.min(room - others).max(lo.min(room - others).max(0.0));
            let lo = lo.min(hi);
            let divider = Rect::from_x_y_ranges((inner - 1.0)..=(inner + 1.0), rect.y_range());
            let response = ResizeHandle::new(
                self.id.with(("column-resize", Some(first))),
                SplitAxis::Horizontal,
                divider,
                &self.style.resize_label,
                self.handle_style(),
            )
            .value(width, lo..=hi)
            .reversed(inward < 0.0)
            .show(ui);
            self.register_interaction(ui.ctx(), response.response.id);
            let next = (width + inward * response.delta).clamp(lo, hi);
            if response.delta != 0.0 && next != width && next > 0.0 {
                self.output.actions.push(if column.iconic {
                    Action::ResizeIconic { panel: first.clone(), width: Some(next) }
                } else {
                    Action::ResizeColumn { panel: first.clone(), width: next }
                });
                // A dock shrunk to fit keeps the widths it was drawn at, so the resized column
                // follows the pointer instead of being scaled again.
                if scaled {
                    for (other, (column, width)) in columns.iter().zip(&widths).enumerate() {
                        let first = column.groups.first().and_then(|g| g.panels.first());
                        if let Some(first) = first.filter(|_| other != index && !column.iconic && *width > 0.0) {
                            self.output.actions.push(Action::ResizeColumn { panel: first.clone(), width: *width });
                        }
                    }
                }
            }
        }
    }

    fn handle_style(&self) -> ResizeHandleStyle {
        ResizeHandleStyle { hovered: self.style.accent, dragged: self.style.accent, focused: self.style.accent }
    }

    /// Width bounds of an expanded column from its panels' limits.
    fn column_bounds(&self, column: &Column<P>) -> (f32, f32) {
        let mut lo = self.style.min_pane;
        let mut hi = f32::INFINITY;
        for panel in column.groups.iter().flat_map(|g| g.panels.iter()) {
            let limits = self.limits(panel);
            lo = lo.max(limits.min.x);
            hi = hi.min(limits.max.x);
        }
        (lo, hi.max(lo))
    }

    /// Least height of an open group: its strip and the tallest minimum of its panels.
    fn group_min(&self, group: &Group<P>) -> f32 {
        let body = group.panels.iter().map(|p| self.limits(p).min.y).fold(self.style.min_pane, f32::max);
        self.style.tab_height + body
    }

    fn column(&mut self, ui: &mut Ui, column: &Column<P>, first: &P, rect: Rect, edge: Edge) {
        if !usable(rect) {
            return;
        }
        self.record_column_float(column.groups.iter().flat_map(|g| g.panels.iter().cloned()).collect(), rect);
        let bar = Rect::from_min_max(rect.min, pos2(rect.right(), (rect.top() + self.style.column_bar_height).min(rect.bottom())));
        self.column_bar(ui, column, first, bar, rect, edge);
        let body = Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max);
        if column.iconic && usable(body) {
            self.iconic_column(ui, column, body, edge);
        }
        if column.iconic || !usable(body) {
            ui.painter().rect_stroke(rect, 0.0, self.style.border, StrokeKind::Inside);
            return;
        }
        let sizes: Vec<_> = column.groups.iter().map(|g| (g.minimized, g.height, self.group_min(g))).collect();
        let heights = group_heights(&sizes, self.style.tab_height, body.height());
        let total: f32 = heights.iter().sum();
        let start = self.targets.len();
        if total > body.height() + 0.5 {
            let mut child =
                ui.new_child(UiBuilder::new().id_salt(("dock-column", first)).max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
            child.set_clip_rect(body.intersect(ui.clip_rect()));
            egui::ScrollArea::vertical()
                .id_salt(self.id.with(("column-scroll", first)))
                .auto_shrink([false, false])
                .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR | egui::scroll_area::ScrollSource::MOUSE_WHEEL)
                .show(&mut child, |ui| {
                    let (content, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
                    self.column_groups(ui, column, &heights, content, body);
                });
        } else {
            self.column_groups(ui, column, &heights, body, body);
        }
        // A group scrolled out of view takes nothing.
        for target in self.targets.iter_mut().skip(start) {
            target.rect = target.rect.intersect(body);
            target.tab_strip = target.tab_strip.map(|strip| strip.intersect(body));
            for (_, tab) in &mut target.tabs {
                *tab = tab.intersect(body);
            }
        }
        ui.painter().rect_stroke(rect, 0.0, self.style.border, StrokeKind::Inside);
    }

    /// Groups top to bottom in `area` (the scrolled content when the column scrolls), their
    /// seams and bands clipped to the visible `view`, and the height dividers.
    fn column_groups(&mut self, ui: &mut Ui, column: &Column<P>, heights: &[f32], area: Rect, view: Rect) {
        let mut top = area.top();
        let mut rects = Vec::new();
        for (group, height) in column.groups.iter().zip(heights) {
            let rect = Rect::from_min_max(pos2(area.left(), top), pos2(area.right(), top + height));
            self.group(ui, &group.panels, group.active, group.minimized, rect, GroupKind::Dock);
            rects.push(rect);
            top += height;
        }
        let xs = view.x_range();
        let half = self.style.seam_band * 0.5;
        let push = |targets: &mut Vec<DockDrop<P>>, target: DockTarget<P>, band: Rect, line: Rect| {
            let band = band.intersect(view);
            if usable(band) {
                targets.push(DockDrop { target, band, line: line.intersect(view.expand(2.0)) });
            }
        };
        let mut previous: Option<&P> = None;
        for (group, rect) in column.groups.iter().zip(&rects) {
            let Some(anchor) = group.panels.first() else { continue };
            if let Some(above) = previous {
                let band = Rect::from_x_y_ranges(xs, (rect.top() - half)..=(rect.top() + half));
                push(&mut self.dock_targets, DockTarget::Seam { anchor: above.clone() }, band, horizontal_line(rect.top(), xs));
            }
            if !group.minimized {
                let body = Rect::from_min_max(pos2(rect.left(), (rect.top() + self.style.tab_height).min(rect.bottom())), rect.max);
                let quarter = body.height() * 0.25;
                let upper = Rect::from_x_y_ranges(xs, body.top()..=(body.top() + quarter));
                let lower = Rect::from_x_y_ranges(xs, (body.bottom() - quarter)..=body.bottom());
                push(&mut self.dock_targets, DockTarget::GroupBand { anchor: anchor.clone(), below: false }, upper, horizontal_line(rect.top(), xs));
                push(
                    &mut self.dock_targets,
                    DockTarget::GroupBand { anchor: anchor.clone(), below: true },
                    lower,
                    horizontal_line(rect.bottom(), xs),
                );
            }
            previous = Some(anchor);
        }
        // The room under the last group, when every group is minimized, stacks below it.
        if let Some(last) = previous
            && top < view.bottom()
        {
            let band = Rect::from_x_y_ranges(xs, top..=view.bottom());
            push(&mut self.dock_targets, DockTarget::GroupBand { anchor: last.clone(), below: true }, band, horizontal_line(top, xs));
        }
        // Height dividers between an open group and the next open one, after the groups so
        // their hit areas win at the boundary. The heights of the groups a drag changes are
        // saved: the upper group, and the lower groups that give way in turn, or the reverse.
        let open: Vec<usize> = (0..column.groups.len()).filter(|i| column.groups.get(*i).is_some_and(|g| !g.minimized)).collect();
        let row: Vec<RowItem> = open
            .iter()
            .filter_map(|i| Some(RowItem { size: *heights.get(*i)?, min: self.group_min(column.groups.get(*i)?), max: f32::INFINITY }))
            .collect();
        for (index, pair) in open.windows(2).enumerate() {
            let &[i, _] = pair else { continue };
            let (Some(upper), Some(rect), Some(a)) = (column.groups.get(i), rects.get(i), heights.get(i)) else { continue };
            let Some(first) = upper.panels.first() else { continue };
            let (lo, hi) = row_range(&row, index);
            let divider = Rect::from_x_y_ranges(rect.x_range(), (rect.bottom() - 1.0)..=(rect.bottom() + 1.0));
            let response = ResizeHandle::new(
                self.id.with(("group-resize", Some(first))),
                SplitAxis::Vertical,
                divider,
                &self.style.resize_label,
                self.handle_style(),
            )
            .value(*a, (a + lo)..=(a + hi))
            .show(ui);
            self.register_interaction(ui.ctx(), response.response.id);
            if response.delta == 0.0 {
                continue;
            }
            for ((group, item), next) in open.iter().filter_map(|g| column.groups.get(*g)).zip(&row).zip(row_drag(&row, index, response.delta)) {
                if let Some(panel) = group.panels.first().filter(|_| next != item.size) {
                    self.output.actions.push(Action::ResizeGroup { panel: panel.clone(), height: Some(next) });
                }
            }
        }
    }

    /// The column bar: the »/« button at its outer end, the rest drags the column (S6),
    /// toggles iconic on double-click (M2) and opens the column menu (U1).
    fn column_bar(&mut self, ui: &mut Ui, column: &Column<P>, first: &P, bar: Rect, rect: Rect, edge: Edge) {
        ui.painter().rect_filled(bar, 0.0, self.style.tab_background);
        let width = COLLAPSE_WIDTH.min(bar.width());
        let (button, grip) = match edge {
            Edge::Left => (
                Rect::from_min_max(bar.min, pos2(bar.left() + width, bar.bottom())),
                Rect::from_min_max(pos2(bar.left() + width, bar.top()), bar.max),
            ),
            Edge::Right => (
                Rect::from_min_max(pos2(bar.right() - width, bar.top()), bar.max),
                Rect::from_min_max(bar.min, pos2(bar.right() - width, bar.bottom())),
            ),
        };
        let label = if column.iconic { self.style.expand_panels_label.clone() } else { self.style.collapse_to_icons_label.clone() };
        if usable(button) {
            let response = ui.interact(button, self.id.with(("column-collapse", Some(first))), Sense::click());
            self.register_interaction(ui.ctx(), response.id);
            let button_label = if column.iconic { &self.style.expand_column_label } else { &self.style.collapse_column_label };
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), button_label));
            // The arrows point where the column goes: towards the window edge when it collapses.
            let glyph = if (edge == Edge::Left) != column.iconic { "«" } else { "»" };
            let color = if response.hovered() || response.has_focus() { self.style.text } else { self.style.inactive_text };
            ui.painter().with_clip_rect(button).text(button.center(), egui::Align2::CENTER_CENTER, glyph, self.style.font.clone(), color);
            if response.clicked() {
                self.output.actions.push(Action::SetIconic { panel: first.clone(), iconic: !column.iconic });
            }
        }
        if !usable(grip) {
            return;
        }
        let movable = column.groups.iter().flat_map(|g| g.panels.iter()).all(|p| self.permissions(p).movable);
        let response = ui.interact(grip, self.id.with(("column-bar", Some(first))), Sense::click_and_drag());
        let response = if movable { response.on_hover_cursor(egui::CursorIcon::Grab) } else { response };
        self.register_interaction(ui.ctx(), response.id);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &self.style.move_column_label));
        if movable && response.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).or_else(|| ui.ctx().pointer_interact_pos()).unwrap_or(grip.center());
            let drag = GroupDrag {
                panel: first.clone(),
                original: [rect.left(), rect.top(), rect.width(), rect.height()],
                gesture: GroupGesture::Column,
                members: column.groups.iter().flat_map(|g| g.panels.iter().cloned()).collect(),
                origin,
                source: rect,
            };
            ui.ctx().data_mut(|d| d.insert_temp(self.id.with("group-drag"), drag));
        }
        if response.double_clicked() {
            self.output.actions.push(Action::SetIconic { panel: first.clone(), iconic: !column.iconic });
        }
        if response.has_focus() || response.hovered() {
            ui.painter().with_clip_rect(grip).rect_stroke(
                grip.shrink(1.0),
                0.0,
                Stroke::new(1.0, self.style.accent.gamma_multiply(0.5)),
                StrokeKind::Inside,
            );
        }
        let keyboard = response.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));
        let command = if response.secondary_clicked() || keyboard {
            Some(egui::SetOpenCommand::Bool(true))
        } else if response.clicked() {
            Some(egui::SetOpenCommand::Bool(false))
        } else {
            None
        };
        egui::Popup::menu(&response).open_memory(command).show(|ui| {
            if self.menu_action(ui, &label) {
                self.output.actions.push(Action::SetIconic { panel: first.clone(), iconic: !column.iconic });
            }
            if self.auto_collapse_item(ui) {
                self.output.events.push(DockEvent::ToggleAutoCollapse);
            }
        });
    }

    /// Whether a floating window drawn this frame (other than the one following the pointer)
    /// or a host overlay covers `pointer`. Covered dock targets take nothing.
    fn dock_covered(&self, pointer: Pos2, moving: Option<&P>) -> bool {
        self.blockers.iter().any(|r| r.contains(pointer))
            || self.iconic.covers(pointer)
            || self.frame.windows.iter().any(|window| window.window.contains(pointer) && !moving.is_some_and(|p| window.members.contains(p)))
    }

    /// Dock targets, with the new-dock bands along the docked area's left and right edges that
    /// have no dock yet.
    fn dock_candidates(&self) -> Vec<DockDrop<P>> {
        let mut candidates = self.dock_targets.clone();
        if !self.style.new_dock_zones || !usable(self.workspace) {
            return candidates;
        }
        let rtl = self.style.right_to_left;
        let ys = self.workspace.y_range();
        for edge in [Edge::Left, Edge::Right] {
            let side = side_at(edge, rtl);
            if self.layout.dock_path(side).is_some() {
                continue;
            }
            let (x, inward) = match edge {
                Edge::Left => (self.workspace.left(), self.style.new_dock_band),
                Edge::Right => (self.workspace.right(), -self.style.new_dock_band),
            };
            let band = Rect::from_x_y_ranges(x.min(x + inward)..=x.max(x + inward), ys);
            let line = vertical_line(x + inward.signum() * 2.0, ys);
            candidates.push(DockDrop { target: DockTarget::NewDock { side }, band, line });
        }
        candidates
    }

    /// The dock target a whole group or column drag would take at `pointer`, with the action
    /// and its line. A target that leaves the layout unchanged is a no-op: `None` action.
    pub(super) fn dock_group_proposal(&self, drag: &GroupDrag<P>, pointer: Pos2) -> Option<(Option<Action<P>>, Rect)> {
        let moving = (drag.gesture == GroupGesture::Window).then_some(&drag.panel);
        if self.dock_covered(pointer, moving) {
            return None;
        }
        let candidates = self.dock_candidates();
        let drop = resolve_dock(pointer, &candidates)?;
        let action = group_action(drag, drop.target.to());
        self.checked(action).map(|action| (action, drop.line))
    }

    /// The dock target a single tab drag would take: a new group, column or dock of that panel.
    /// A panel alone in its group takes the group's state to a new dock; one that shares its
    /// group leaves the other tabs behind (P6).
    pub(super) fn dock_tab_proposal(&self, panel: &P, pointer: Pos2) -> Option<(Option<Action<P>>, Rect)> {
        if self.dock_covered(pointer, None) {
            return None;
        }
        let candidates = self.dock_candidates();
        let drop = resolve_dock(pointer, &candidates)?;
        let action = match drop.target.to() {
            // A target named by the dragged panel is its own group or column: another panel
            // there names the same one. With none, the drop leaves the panel where it is.
            Target::At { anchor, placement } if &anchor == panel => {
                let unit = if matches!(placement, Placement::Column { .. }) { Unit::Column } else { Unit::Group };
                match self.layout.members(panel, unit).ok()?.into_iter().find(|p| p != panel) {
                    Some(anchor) => Action::Move { panel: panel.clone(), anchor, placement },
                    None => return Some((None, drop.line)),
                }
            }
            Target::At { anchor, placement } => Action::Move { panel: panel.clone(), anchor, placement },
            to @ Target::NewDock { .. } if self.place(panel).and_then(|place| self.layout.group(place)).is_some_and(|g| g.panels.len() == 1) => {
                Action::MoveGroup { panel: panel.clone(), to }
            }
            to @ Target::NewDock { .. } => Action::MovePanel { panel: panel.clone(), to },
            Target::Float { .. } => return None,
        };
        self.checked(action).map(|action| (action, drop.line))
    }

    /// `Some(None)` when the action is valid but changes nothing, `None` when it is invalid.
    fn checked(&self, action: Action<P>) -> Option<Option<Action<P>>> {
        let mut candidate = self.layout.clone();
        candidate.apply_with_permissions(action.clone(), |p| self.permissions(p)).ok()?;
        Some((candidate != *self.layout).then_some(action))
    }

    /// The insertion line of a dock target.
    pub(super) fn paint_dock_line(&self, ctx: &Context, line: Rect) {
        if usable(line) {
            let painter = self.preview_painter(ctx);
            self.iconic.outline(&painter, line, self.style);
            painter.rect_filled(line, 1.0, self.style.accent);
        }
    }
}

/// The move a group gesture makes: the column for a column bar, every group for a window's
/// title bar, else the group.
pub(super) fn group_action<P: Clone>(drag: &GroupDrag<P>, to: Target<P>) -> Action<P> {
    match drag.gesture {
        GroupGesture::Column => Action::MoveColumn { panel: drag.panel.clone(), to },
        GroupGesture::Window => Action::MoveWindow { panel: drag.panel.clone(), to },
        GroupGesture::Group => Action::MoveGroup { panel: drag.panel.clone(), to },
    }
}
