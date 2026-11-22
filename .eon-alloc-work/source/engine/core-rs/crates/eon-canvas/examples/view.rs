//! Opens the layout view on a small control-flow-shaped graph. Run with `cargo run -p eon-canvas --example view`.

fn main() -> eframe::Result {
    let edges = [(0, 1), (0, 2), (1, 3), (2, 3), (3, 4), (0, 4)];
    let layout = eon_canvas::layered(5, &edges).expect("the sample graph is acyclic");
    eon_canvas::view::run(layout)
}
