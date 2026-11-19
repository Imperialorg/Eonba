//! The egui view paints one box and one label per node, and one line per edge segment. Runs without a window.

use eon_canvas::diff::{match_blocks, Decision, Review};
use eon_canvas::layered;
use eon_canvas::view::{control_rect, paint_diff, paint_layout, review_controls};
use eon_cyber::cpg::{function_blocks, Lang};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, RawInput, Shape, vec2};

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

fn review_frame(
    ctx: &egui::Context,
    events: Vec<Event>,
    old: &[eon_cyber::cpg::FunctionBlock],
    new: &[eon_cyber::cpg::FunctionBlock],
    pairings: &[eon_canvas::diff::Pairing],
    review: &mut Review,
) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
        events,
        ..RawInput::default()
    };
    let mut output = ctx.run_ui(input, |ui| review_controls(ui, Pos2::ZERO, old, new, pairings, review));
    output.textures_delta.clear();
}

fn click(
    ctx: &egui::Context,
    at: Pos2,
    old: &[eon_cyber::cpg::FunctionBlock],
    new: &[eon_cyber::cpg::FunctionBlock],
    pairings: &[eon_canvas::diff::Pairing],
    review: &mut Review,
) {
    review_frame(ctx, vec![], old, new, pairings, review);
    review_frame(ctx, vec![Event::PointerMoved(at)], old, new, pairings, review);
    let press = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    review_frame(ctx, vec![press(true)], old, new, pairings, review);
    review_frame(ctx, vec![press(false)], old, new, pairings, review);
}

#[test]
fn apply_and_nack_buttons_record_the_reviewer_decision() {
    let old = function_blocks(Lang::Rust, "fn a() { 1 }\nfn b() { 2 }\n").unwrap();
    let new = function_blocks(Lang::Rust, "fn a() { 3 }\nfn b() { 4 }\n").unwrap();
    let pairings = match_blocks(&old, &new);
    let mut review = Review::new(&pairings);
    let ctx = egui::Context::default();

    click(&ctx, control_rect(Pos2::ZERO, 0, false).center(), &old, &new, &pairings, &mut review);
    click(&ctx, control_rect(Pos2::ZERO, 1, true).center(), &old, &new, &pairings, &mut review);

    assert_eq!(review.decision(0), Some(Decision::Applied));
    assert_eq!(review.decision(1), Some(Decision::Nacked));
    assert_eq!(review.pending(), 0);
}
