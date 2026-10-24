use egui::{Pos2, Rangef, Rect, Vec2, pos2};

use super::{Placement, Side, Target, Zone};

/// One visible group, supplied in back-to-front order to [`resolve_drop`]. An empty tab strip
/// or a group that rejects tabs may still accept side splits, e.g. a protected document canvas.
#[derive(Clone, Debug)]
pub struct DropTarget<P> {
    pub anchor: P,
    pub rect: Rect,
    pub tab_strip: Option<Rect>,
    pub tabs: Vec<(P, Rect)>,
    pub allow_splits: bool,
    pub accepts_tabs: bool,
    /// The strip runs right to left: the first tab is rightmost, and an insertion slot sits
    /// at the right edge of the tab it precedes.
    pub right_to_left: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DropProposal<P> {
    pub anchor: P,
    pub placement: Placement<P>,
    pub preview: Rect,
    pub insertion: Option<Rect>,
    /// The pointer addresses the tab's current insertion slot.
    pub noop: bool,
}

pub(super) fn usable(rect: Rect) -> bool {
    rect.is_finite() && rect.width() > 0.0 && rect.height() > 0.0 && rect.width() <= 1_000_000.0 && rect.height() <= 1_000_000.0
}

/// Tab header (the top `tab_height` of `rect`) is the center; otherwise the nearest side zone of
/// the body below it wins. Every side zone reaches the same depth into the body: `depth`, or a
/// quarter of the body's smaller side when that is less, so a body always keeps a center and a
/// narrow body's top and bottom zones are no deeper than its left and right ones. A `depth` of
/// zero or less leaves only the center. Outside, non-finite, empty and inverted rectangles have
/// no target.
pub fn drop_zone(rect: Rect, tab_height: f32, depth: f32, pointer: Pos2) -> Option<Zone> {
    if !usable(rect) || !pointer.is_finite() || !rect.contains(pointer) || !tab_height.is_finite() || tab_height < 0.0 {
        return None;
    }
    let body = body(rect, tab_height);
    if pointer.y < body.top() || !usable(body) {
        return Some(Zone::Center);
    }
    let Vec2 { x, y } = side_depths(body, depth);
    let sides = [
        (pointer.x - body.left(), x, Zone::Left),
        (body.right() - pointer.x, x, Zone::Right),
        (pointer.y - body.top(), y, Zone::Top),
        (body.bottom() - pointer.y, y, Zone::Bottom),
    ];
    let nearest =
        sides.into_iter().filter(|(_, depth, _)| *depth > 0.0).map(|(gap, depth, zone)| (gap / depth, zone)).min_by(|a, b| a.0.total_cmp(&b.0));
    Some(match nearest {
        Some((distance, zone)) if distance <= 1.0 => zone,
        _ => Zone::Center,
    })
}

/// The part of a group drawn at `rect` below its tab header.
fn body(rect: Rect, tab_height: f32) -> Rect {
    Rect::from_min_max(pos2(rect.left(), rect.top() + tab_height.min(rect.height())), rect.max)
}

/// How deep the side zones of `body` reach in from its left and right edges (`x`) and its top
/// and bottom edges (`y`): the same on all four sides.
fn side_depths(body: Rect, depth: f32) -> Vec2 {
    let depth = if depth > 0.0 { depth } else { 0.0 };
    Vec2::splat(depth.min(body.width().min(body.height()) / 4.0))
}

/// The outline of a drop zone of the group drawn at `rect`, as [`drop_zone`] finds it with the
/// same `tab_height` and `depth`: a side's trapezoid (its outer edge the whole side of the body,
/// its inner edge on the center rectangle), or the center rectangle. The tab header, a grouping
/// zone too, is not part of any shape. None for a side without depth.
pub fn zone_shape(rect: Rect, tab_height: f32, depth: f32, zone: Zone) -> Option<Vec<Pos2>> {
    if !usable(rect) || !tab_height.is_finite() || tab_height < 0.0 {
        return None;
    }
    let body = body(rect, tab_height);
    let depths = side_depths(body, depth);
    let flat = match zone {
        Zone::Left | Zone::Right => depths.x <= 0.0,
        Zone::Top | Zone::Bottom => depths.y <= 0.0,
        Zone::Center => false,
    };
    if !usable(body) || flat {
        return None;
    }
    let inner = body.shrink2(depths);
    Some(match zone {
        Zone::Left => vec![body.left_top(), inner.left_top(), inner.left_bottom(), body.left_bottom()],
        Zone::Right => vec![inner.right_top(), body.right_top(), body.right_bottom(), inner.right_bottom()],
        Zone::Top => vec![body.left_top(), body.right_top(), inner.right_top(), inner.left_top()],
        Zone::Bottom => vec![inner.left_bottom(), inner.right_bottom(), body.right_bottom(), body.left_bottom()],
        Zone::Center => vec![inner.left_top(), inner.right_top(), inner.right_bottom(), inner.left_bottom()],
    })
}

/// Height of the tab header of `target` that [`resolve_drop`] treats as a grouping zone: from the
/// top of its rect to the bottom of its tab strip.
pub(super) fn header_height<P>(target: &DropTarget<P>) -> f32 {
    target.tab_strip.map(|strip| strip.intersect(target.rect)).filter(|strip| usable(*strip)).map_or(0.0, |strip| strip.bottom() - target.rect.top())
}

/// Preview of the region occupied after a split or group merge.
pub fn zone_rect(rect: Rect, zone: Zone) -> Option<Rect> {
    if !usable(rect) {
        return None;
    }
    let center = rect.min + rect.size() * 0.5;
    Some(match zone {
        Zone::Center => rect,
        Zone::Left => Rect::from_min_max(rect.min, pos2(center.x, rect.bottom())),
        Zone::Right => Rect::from_min_max(pos2(center.x, rect.top()), rect.max),
        Zone::Top => Rect::from_min_max(rect.min, pos2(rect.right(), center.y)),
        Zone::Bottom => Rect::from_min_max(pos2(rect.left(), center.y), rect.max),
    })
}

/// Bounded insertion marker for a logical slot in a measured tab strip. Hidden tabs
/// retain their order; their slot uses the next visible boundary or the visible strip end.
pub fn tab_insertion_marker<P: Eq>(target: &DropTarget<P>, before: Option<&P>) -> Option<Rect> {
    if target.tabs.len() > super::MAX_PANELS || !target.accepts_tabs || !usable(target.rect) {
        return None;
    }
    let strip = target.tab_strip?.intersect(target.rect);
    if !usable(strip) {
        return None;
    }
    let index = before.and_then(|before| target.tabs.iter().position(|(panel, _)| panel == before));
    let rtl = target.right_to_left;
    let leading = |rect: &Rect| if rtl { rect.right() } else { rect.left() };
    let visible = || target.tabs.iter().filter(|(_, rect)| usable(*rect)).map(|(_, rect)| rect);
    let x = index
        .and_then(|index| target.tabs.get(index..))
        .and_then(|tabs| tabs.iter().find(|(_, rect)| usable(*rect)).map(|(_, rect)| leading(rect)))
        .unwrap_or_else(|| {
            if rtl {
                visible().map(|rect| rect.left()).fold(strip.right(), f32::min).max(strip.left())
            } else {
                visible().map(|rect| rect.right()).fold(strip.left(), f32::max).min(strip.right())
            }
        });
    let width = strip.width().min(2.0);
    let left = (x - width * 0.5).max(strip.left()).min((strip.right() - width).max(strip.left()));
    Some(Rect::from_min_max(pos2(left, strip.top()), pos2((left + width).min(strip.right()), strip.bottom())))
}

/// Resolve a pointer against visible groups. The frontmost containing group occludes groups
/// behind it even if it rejects the drop. A strip reorders by midpoint; the body of the source
/// group is a no-op at center. The body's side zones are [`drop_zone`]'s with `zone_depth` (see
/// [`crate::docking::DockStyle::zone_depth`]): one depth on all four sides, at most a quarter of
/// the body's smaller side. Floating groups normally set `allow_splits` to false.
pub fn resolve_drop<P: Clone + Eq>(panel: &P, pointer: Pos2, targets: &[DropTarget<P>], zone_depth: f32) -> Option<DropProposal<P>> {
    if !pointer.is_finite() || targets.len() > super::MAX_PANELS {
        return None;
    }
    let target = targets.iter().rev().find(|target| usable(target.rect) && target.rect.contains(pointer))?;
    if target.tabs.len() > super::MAX_PANELS {
        return None;
    }
    let anchor = if &target.anchor == panel { target.tabs.iter().find(|(p, _)| p != panel).map(|(p, _)| p).unwrap_or(panel) } else { &target.anchor };
    let strip = target.tab_strip.map(|strip| strip.intersect(target.rect)).filter(|r| usable(*r) && r.contains(pointer));
    if strip.is_some() {
        if !target.accepts_tabs {
            return None;
        }
        let ahead = |r: &Rect| if target.right_to_left { pointer.x > r.center().x } else { pointer.x < r.center().x };
        let before = target.tabs.iter().filter(|(_, r)| usable(*r)).find(|(_, r)| ahead(r));
        let before_panel = before.map(|(p, _)| p.clone());
        let insertion = tab_insertion_marker(target, before_panel.as_ref())?;
        let old_order: Vec<_> = target.tabs.iter().map(|(p, _)| p).collect();
        let mut order = old_order.clone();
        let noop = if before_panel.as_ref() == Some(panel) {
            old_order.contains(&panel)
        } else if old_order.contains(&panel) {
            order.retain(|p| *p != panel);
            let at = before_panel.as_ref().and_then(|before| order.iter().position(|p| *p == before)).unwrap_or(order.len());
            order.insert(at, panel);
            order == old_order
        } else {
            false
        };
        return Some(DropProposal {
            anchor: anchor.clone(),
            placement: Placement::Tab { before: before_panel },
            preview: target.rect,
            insertion: Some(insertion),
            noop,
        });
    }
    let zone = if target.allow_splits { drop_zone(target.rect, header_height(target), zone_depth, pointer)? } else { Zone::Center };
    if zone == Zone::Center && target.accepts_tabs && target.tabs.iter().any(|(p, _)| p == panel) {
        return Some(DropProposal {
            anchor: anchor.clone(),
            placement: Placement::Tab { before: Some(panel.clone()) },
            preview: target.rect,
            insertion: None,
            noop: true,
        });
    }
    if anchor == panel {
        return None;
    }
    if zone == Zone::Center {
        if !target.accepts_tabs || target.tabs.iter().any(|(p, _)| p == panel) {
            return None;
        }
    } else if !target.allow_splits {
        return None;
    }
    Some(DropProposal {
        anchor: anchor.clone(),
        placement: Placement::Split(zone),
        preview: zone_rect(target.rect, zone)?,
        insertion: None,
        noop: false,
    })
}

/// [D] A dock drop target, built once per frame next to the [`DropTarget`]s. The pointer selects
/// it inside `band`; the preview is the line `line`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DockDrop<P> {
    pub(super) target: DockTarget<P>,
    pub(super) band: Rect,
    pub(super) line: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum DockTarget<P> {
    /// Column edge: a new column beside the column of `anchor`, further from the window edge
    /// when `after` (P5).
    ColumnEdge { anchor: P, after: bool },
    /// The seam between two stacked groups: a new group below the group of `anchor` (P4).
    Seam { anchor: P },
    /// The top or bottom quarter of a group body, or the free room under a column's last group (P3).
    GroupBand { anchor: P, below: bool },
    /// A window edge without a dock, or an empty dock (P5, P6).
    NewDock { side: Side },
}

impl<P: Clone> DockTarget<P> {
    pub(super) fn to(&self) -> Target<P> {
        match self {
            Self::ColumnEdge { anchor, after } => Target::At { anchor: anchor.clone(), placement: Placement::Column { after: *after } },
            Self::Seam { anchor } => Target::At { anchor: anchor.clone(), placement: Placement::Group { below: true, height: None } },
            Self::GroupBand { anchor, below } => Target::At { anchor: anchor.clone(), placement: Placement::Group { below: *below, height: None } },
            Self::NewDock { side } => Target::NewDock { side: *side },
        }
    }

    /// Rank in resolution order: column edges reach past their columns, so they come first.
    fn rank(&self) -> u8 {
        match self {
            Self::ColumnEdge { .. } => 0,
            Self::Seam { .. } => 1,
            Self::GroupBand { .. } => 2,
            Self::NewDock { .. } => 3,
        }
    }
}

/// The dock target under the pointer: column edges, then seams, then group bands, then new docks.
pub(super) fn resolve_dock<P: Clone>(pointer: Pos2, targets: &[DockDrop<P>]) -> Option<&DockDrop<P>> {
    if !pointer.is_finite() {
        return None;
    }
    targets.iter().filter(|t| usable(t.band) && t.band.contains(pointer)).min_by_key(|t| t.target.rank())
}

/// Width of the insertion lines dock targets preview with.
const LINE: f32 = 4.0;
/// How far a column edge band reaches into the column, and past the edge out of it.
const EDGE_IN: f32 = 6.0;
const EDGE_OUT: f32 = 10.0;

/// Band and line of a column edge at `x`. `inward` is +1 when the column lies right of the edge,
/// -1 when it lies left of it. The band reaches further out of the column than into it.
pub(super) fn column_edge(x: f32, inward: f32, ys: Rangef) -> (Rect, Rect) {
    let (a, b) = (x + inward * EDGE_IN, x - inward * EDGE_OUT);
    (Rect::from_x_y_ranges(a.min(b)..=a.max(b), ys), vertical_line(x, ys))
}

pub(super) fn vertical_line(x: f32, ys: Rangef) -> Rect {
    Rect::from_x_y_ranges((x - LINE * 0.5)..=(x + LINE * 0.5), ys)
}

pub(super) fn horizontal_line(y: f32, xs: Rangef) -> Rect {
    Rect::from_x_y_ranges(xs, (y - LINE * 0.5)..=(y + LINE * 0.5))
}
