//! egui view of a layout. Boxes, edge segments, and node labels are painted with egui's painter, using the same geometry as
//! the offscreen renderer. [`LayoutView`] is the window shell, run with `eframe`.

use eframe::egui;
use eon_cyber::cpg::FunctionBlock;

use crate::diff::{Pairing, Status};
use crate::render::{node_centre, NODE_HEIGHT, NODE_WIDTH};
use crate::Layout;

const NODE_FILL: egui::Color32 = egui::Color32::from_rgb(51, 128, 230);
const EDGE_STROKE: egui::Color32 = egui::Color32::from_rgb(230, 179, 51);
const LABEL: egui::Color32 = egui::Color32::WHITE;
const BLOCK_WIDTH: f32 = 360.0;
const BLOCK_HEIGHT: f32 = 44.0;
const ROW_HEIGHT: f32 = 56.0;
const NEW_COLUMN_X: f32 = 460.0;
const RIBBON: egui::Color32 = egui::Color32::from_rgba_premultiplied(120, 120, 130, 150);

fn status_fill(status: Status) -> egui::Color32 {
    match status {
        Status::Unchanged => egui::Color32::from_rgb(90, 90, 100),
        Status::Changed => egui::Color32::from_rgb(230, 179, 51),
        Status::Added => egui::Color32::from_rgb(80, 190, 120),
        Status::Removed => egui::Color32::from_rgb(220, 80, 80),
    }
}

/// Paints the structural diff: old functions on the left, new on the right, and a ribbon between each matched pair.
pub fn paint_diff(
    painter: &egui::Painter,
    origin: egui::Pos2,
    old: &[FunctionBlock],
    new: &[FunctionBlock],
    pairings: &[Pairing],
) {
    let block_rect = |x: f32, row: usize| {
        let top_left = origin + egui::vec2(x, 40.0 + row as f32 * ROW_HEIGHT);
        egui::Rect::from_min_size(top_left, egui::vec2(BLOCK_WIDTH, BLOCK_HEIGHT))
    };
    for pairing in pairings {
        let old_rect = pairing.old.map(|i| block_rect(40.0, i));
        let new_rect = pairing.new.map(|j| block_rect(NEW_COLUMN_X, j));
        if let (Some(from), Some(to)) = (old_rect, new_rect) {
            let start = from.right_center();
            let end = to.left_center();
            let reach = (end.x - start.x) / 2.0;
            let points = [
                start,
                start + egui::vec2(reach, 0.0),
                end - egui::vec2(reach, 0.0),
                end,
            ];
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::new(1.5, RIBBON),
            ));
        }
        for (rect, index, blocks) in [(old_rect, pairing.old, old), (new_rect, pairing.new, new)] {
            if let (Some(rect), Some(index)) = (rect, index) {
                painter.rect_filled(rect, 4.0, status_fill(pairing.status));
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &blocks[index].name,
                    egui::FontId::monospace(13.0),
                    LABEL,
                );
            }
        }
    }
}

pub fn paint_layout(painter: &egui::Painter, origin: egui::Pos2, layout: &Layout) {
    let at = |p: [f32; 2]| origin + egui::vec2(p[0], p[1]);
    for path in &layout.edge_paths {
        for pair in path.windows(2) {
            let a = node_centre(pair[0].0, pair[0].1);
            let b = node_centre(pair[1].0, pair[1].1);
            painter.line_segment([at(a), at(b)], egui::Stroke::new(3.0, EDGE_STROKE));
        }
    }
    for (index, (&rank, &slot)) in layout.rank.iter().zip(&layout.slot).enumerate() {
        let centre = node_centre(rank, slot);
        let rect = egui::Rect::from_center_size(at(centre), egui::vec2(NODE_WIDTH, NODE_HEIGHT));
        painter.rect_filled(rect, 4.0, NODE_FILL);
        painter.text(
            at(centre),
            egui::Align2::CENTER_CENTER,
            format!("n{index}"),
            egui::FontId::proportional(14.0),
            LABEL,
        );
    }
}

pub struct LayoutView {
    layout: Layout,
}

impl LayoutView {
    pub fn new(layout: Layout) -> Self {
        Self { layout }
    }
}

impl eframe::App for LayoutView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let origin = ui.max_rect().min;
        paint_layout(ui.painter(), origin, &self.layout);
    }
}

/// Opens a window that shows `layout` until it is closed.
pub fn run(layout: Layout) -> eframe::Result {
    eframe::run_native(
        "eon-canvas",
        eframe::NativeOptions::default(),
        Box::new(move |_creation| Ok(Box::new(LayoutView::new(layout)))),
    )
}

pub struct DiffView {
    old: Vec<FunctionBlock>,
    new: Vec<FunctionBlock>,
    pairings: Vec<Pairing>,
}

impl eframe::App for DiffView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let origin = ui.max_rect().min;
        paint_diff(ui.painter(), origin, &self.old, &self.new, &self.pairings);
    }
}

/// Opens a window that shows the structural diff of two sources until it is closed.
pub fn run_diff(old: Vec<FunctionBlock>, new: Vec<FunctionBlock>) -> eframe::Result {
    let pairings = crate::diff::match_blocks(&old, &new);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        "eon-canvas diff",
        options,
        Box::new(move |_creation| Ok(Box::new(DiffView { old, new, pairings }))),
    )
}
