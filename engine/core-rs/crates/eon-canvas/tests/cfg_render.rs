//! Read back the same egui scene the operator sees, including the font atlas and arrowheads.

use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
    time::Duration,
};

use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor, capture::CaptureState};
use egui::{Color32, ColorImage, Pos2, Rect, Shape, Vec2, pos2, vec2};
use eon_canvas::cfg::CfgView;

const SIZE: [u32; 2] = [640, 640];

fn render(ctx: &egui::Context, mut output: egui::FullOutput) -> Arc<ColorImage> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = desc.backends.with_env();
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let extent = wgpu::Extent3d {
        width: SIZE[0],
        height: SIZE[1],
        depth_or_array_layers: 1,
    };
    let mut capture = CaptureState::new(&device, extent, format);
    let mut renderer = Renderer::new(&device, format, RendererOptions::PREDICTABLE);
    for (id, deltas) in output.textures_delta.set.drain() {
        for delta in deltas {
            renderer.update_texture(&device, &queue, id, &delta);
        }
    }
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = ScreenDescriptor {
        size_in_pixels: SIZE,
        pixels_per_point: output.pixels_per_point,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let extra = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
    {
        let view = capture.texture.create_view(&Default::default());
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Default::default(),
                depth_slice: None,
            })],
            ..Default::default()
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    let buffer = capture.copy_textures(&device, None, &mut encoder);
    queue.submit(extra.into_iter().chain([encoder.finish()]));
    for id in output.textures_delta.free.drain() {
        renderer.free_texture(&id);
    }
    let (tx, rx) = mpsc::channel();
    capture.read_screen_rgba(
        buffer,
        vec![egui::ScreenshotCallback::new(move |image| {
            tx.send(image).unwrap()
        })],
    );
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

#[test]
fn gcc_graph_renders_all_ids_directed_branches_back_edge_and_self_loop() {
    let view = CfgView::read(&include_bytes!("fixtures/count_groups.cfg")[..]).unwrap();
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            Pos2::ZERO,
            vec2(SIZE[0] as f32, SIZE[1] as f32),
        )),
        ..Default::default()
    };
    let output = ctx.run_ui(input, |ui| view.show(ui));
    // Locate the actual diagram boxes rather than assuming a header height or UI margin.
    let boxes: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Rect(r) if r.fill == Color32::from_rgb(51, 128, 230) => Some(r.rect),
            _ => None,
        })
        .collect();
    let mut nodes = BTreeMap::<u32, Pos2>::new();
    for s in &output.shapes {
        if let Shape::Text(t) = &s.shape
            && let Some(id) = t.galley.job.text.strip_prefix("bb ")
        {
            let rect = boxes
                .iter()
                .find(|r| r.contains(t.visual_bounding_rect().center()))
                .unwrap();
            assert!(s.clip_rect.contains_rect(*rect));
            assert!(nodes.insert(id.parse().unwrap(), rect.center()).is_none());
        }
    }
    assert_eq!(
        nodes.keys().copied().collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(boxes.len(), nodes.len());
    let image = render(&ctx, output);
    let pixel = |p: Pos2| image[(p.x.floor() as usize, p.y.floor() as usize)];
    for &p in nodes.values() {
        // Check for bright glyph ink *inside* each blue block in the read-back pixels.
        let white = (-8..=8)
            .flat_map(|dy| (-25..=25).map(move |dx| p + vec2(dx as f32, dy as f32)))
            .filter(|&p| {
                let c = pixel(p);
                c.r() > 220 && c.g() > 220 && c.b() > 220
            })
            .count();
        assert!(
            white >= 12,
            "missing glyphs at {p:?} ({white} bright pixels)"
        );
        assert_eq!(pixel(p + vec2(-40.0, 0.0)), Color32::from_rgb(51, 128, 230));
    }
    let gold = Color32::from_rgb(230, 179, 51);
    let pink = Color32::from_rgb(236, 110, 142);
    let check = |p: Pos2, want: Color32| {
        let got = pixel(p);
        for (a, b) in got.to_array().into_iter().zip(want.to_array()) {
            assert!(
                a.abs_diff(b) <= 28,
                "pixel at {p:?}: {got:?}, expected {want:?}"
            );
        }
    };
    // Sample off the shaft, within the wide part of each arrowhead.
    let head = |tip: Pos2, direction: Vec2, color| {
        check(tip - direction * 9.0 + direction.rot90() * 2.8, color);
    };
    for (u, v) in [(2, 5), (5, 3), (3, 4), (5, 6), (6, 7), (7, 1)] {
        let start = nodes[&u] + vec2(0.0, 20.0);
        let end = nodes[&v] - vec2(0.0, 20.0);
        check(start.lerp(end, 0.5), gold);
        head(end, (end - start).normalized(), gold);
    }
    // bb 3 -> bb 3: to the right of the box and back down into its top edge.
    let b3 = nodes[&3];
    check(b3 + vec2(74.0, -16.0), pink);
    head(b3 + vec2(46.0, -20.0), Vec2::DOWN, pink);
    // bb 4 -> bb 5: outside all columns and up into bb 5's lower edge.
    let b4 = nodes[&4];
    let b5 = nodes[&5];
    let right = boxes.iter().map(Rect::right).fold(0.0, f32::max);
    check(pos2(right + 24.0, (b4.y + b5.y) * 0.5), pink);
    check(b4 + vec2(30.0, -27.0), pink);
    head(b5 + vec2(30.0, 20.0), Vec2::UP, pink);
}
