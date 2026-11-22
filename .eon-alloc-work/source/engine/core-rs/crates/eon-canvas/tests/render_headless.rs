//! Headless render check on the GPU adapter. Needs a Vulkan, Metal, or DX12 adapter; CPU-only hosts use lavapipe.

use eon_canvas::layered;
use eon_canvas::render::{image_size, render, Style};

fn close(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| (i32::from(*a) - i32::from(e)).abs() <= 2)
}

#[test]
fn renders_nodes_edges_and_background_at_their_positions() {
    let layout = layered(2, &[(0, 1)]).unwrap();
    assert_eq!(image_size(&layout), (240, 280));

    let image = render(&layout, &Style::default()).expect("a GPU adapter is required (lavapipe on CPU hosts)");
    assert_eq!((image.width, image.height), (240, 280));
    assert!(close(image.pixel(120, 90), [51, 128, 230, 255]), "node 0 centre: {:?}", image.pixel(120, 90));
    assert!(close(image.pixel(120, 190), [51, 128, 230, 255]), "node 1 centre: {:?}", image.pixel(120, 190));
    assert!(close(image.pixel(120, 140), [230, 179, 51, 255]), "edge midpoint: {:?}", image.pixel(120, 140));
    assert!(close(image.pixel(5, 5), [13, 13, 20, 255]), "background: {:?}", image.pixel(5, 5));
}
