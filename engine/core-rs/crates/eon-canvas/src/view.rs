//! egui view of a layout. Boxes, edge segments, and node labels are painted with egui's painter, using the same geometry as
//! the offscreen renderer. [`LayoutView`] is the window shell, run with `eframe`.

use eframe::egui;

use crate::render::{node_centre, NODE_HEIGHT, NODE_WIDTH};
use crate::Layout;

const NODE_FILL: egui::Color32 = egui::Color32::from_rgb(51, 128, 230);
const EDGE_STROKE: egui::Color32 = egui::Color32::from_rgb(230, 179, 51);
const LABEL: egui::Color32 = egui::Color32::WHITE;

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
