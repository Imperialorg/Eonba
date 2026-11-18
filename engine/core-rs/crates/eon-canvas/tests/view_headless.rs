//! The egui view paints one box and one label per node, and one line per edge segment. Runs without a window.

use eon_canvas::diff::match_blocks;
use eon_canvas::layered;
use eon_canvas::view::{paint_diff, paint_layout};
use eon_cyber::cpg::{function_blocks, Lang};
use egui::{RawInput, Shape};

#[test]
fn the_view_paints_a_box_and_label_per_node_and_a_segment_per_edge_hop() {
    // The edge 0 -> 2 spans two ranks, so it becomes two segments through a dummy node.
    let layout = layered(3, &[(0, 1), (0, 2), (1, 2)]).unwrap();
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        paint_layout(ui.painter(), egui::pos2(0.0, 0.0), &layout);
    });
    output.textures_delta.clear();

    let shapes: Vec<&Shape> = output.shapes.iter().map(|clipped| &clipped.shape).collect();
    let boxes = shapes.iter().filter(|s| matches!(s, Shape::Rect(_))).count();
    let labels = shapes.iter().filter(|s| matches!(s, Shape::Text(_))).count();
    let segments = shapes.iter().filter(|s| matches!(s, Shape::LineSegment { .. })).count();

    assert_eq!(boxes, 3);
    assert_eq!(labels, 3);
    assert_eq!(segments, 4);
}

#[test]
fn the_diff_paints_two_blocks_and_one_ribbon_per_matched_pair() {
    let old = function_blocks(Lang::Rust, "fn a() { 1 }\nfn b() { 2 }\nfn gone() {}\n").unwrap();
    let new = function_blocks(Lang::Rust, "fn a() { 1 }\nfn b() { 3 }\nfn fresh() {}\n").unwrap();
    let pairings = match_blocks(&old, &new);
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        paint_diff(ui.painter(), egui::pos2(0.0, 0.0), &old, &new, &pairings);
    });
    output.textures_delta.clear();

    let shapes: Vec<&Shape> = output.shapes.iter().map(|clipped| &clipped.shape).collect();
    let boxes = shapes.iter().filter(|s| matches!(s, Shape::Rect(_))).count();
    let ribbons = shapes.iter().filter(|s| matches!(s, Shape::CubicBezier(_))).count();

    // a and b are matched (two boxes each), gone is removed and fresh is added (one box each).
    assert_eq!(boxes, 6);
    assert_eq!(ribbons, 2);
}
