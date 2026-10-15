//! The tray icons drawn at run time, for the platforms whose tray takes pixels instead of an icon name
//! (macOS, Windows). Same shapes as the Linux SVGs in `icons.rs`, on the same 64x64 design grid.
use super::model::State;

/// Straight (not premultiplied) RGBA, row by row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

enum Shape {
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    Round {
        cx: f32,
        cy: f32,
        hw: f32,
        hh: f32,
        r: f32,
    },
}

impl Shape {
    fn contains(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::Circle { cx, cy, r } => (x - cx).hypot(y - cy) <= r,
            Shape::Round { cx, cy, hw, hh, r } => {
                let qx = (x - cx).abs() - (hw - r);
                let qy = (y - cy).abs() - (hh - r);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) <= r
            }
        }
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Shape {
    Shape::Round {
        cx: x + w / 2.0,
        cy: y + h / 2.0,
        hw: w / 2.0,
        hh: h / 2.0,
        r,
    }
}

fn dot(cx: f32, cy: f32, r: f32) -> Shape {
    Shape::Circle { cx, cy, r }
}

/// The glyph of a state, and how much of it shows (the faded states).
fn glyph(state: State) -> (Vec<Shape>, f32) {
    let mut shapes = match state {
        State::Connected => vec![rect(14.0, 29.5, 36.0, 5.0, 2.5)],
        State::Waiting => [26.0, 32.0, 38.0].map(|x| dot(x, 32.0, 2.6)).into(),
        State::Paused => vec![
            rect(26.5, 23.0, 4.5, 18.0, 2.0),
            rect(33.0, 23.0, 4.5, 18.0, 2.0),
        ],
        State::NoDevices | State::Stopped => vec![],
    };
    shapes.push(dot(14.0, 32.0, 9.0));
    shapes.push(dot(50.0, 32.0, 9.0));
    let fade = match state {
        State::NoDevices => 0.55,
        State::Stopped => 0.3,
        _ => 1.0,
    };
    (shapes, fade)
}

const SAMPLES: u32 = 4;

/// Draw a state at `size` x `size` pixels in the colour `ink`; the alpha channel carries the shape.
pub fn render(state: State, badge: bool, size: u32, ink: [u8; 3]) -> Rgba {
    let (shapes, fade) = glyph(state);
    let badge_dot = dot(52.0, 12.0, 10.0);
    let scale = 64.0 / size as f32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let (mut glyph_hits, mut badge_hits) = (0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * scale;
                    if badge && badge_dot.contains(x, y) {
                        badge_hits += 1;
                    } else if shapes.iter().any(|s| s.contains(x, y)) {
                        glyph_hits += 1;
                    }
                }
            }
            let total = (SAMPLES * SAMPLES) as f32;
            let alpha = (glyph_hits as f32 * fade + badge_hits as f32) / total;
            data.extend_from_slice(&ink);
            data.push((alpha * 255.0).round() as u8);
        }
    }
    Rgba {
        w: size,
        h: size,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::model::State;

    const INK: [u8; 3] = [200, 100, 50];
    const ALL: [State; 5] = [
        State::Connected,
        State::Waiting,
        State::NoDevices,
        State::Paused,
        State::Stopped,
    ];

    /// The alpha of the pixel that covers a point of the 64x64 design grid.
    fn alpha_at(img: &Rgba, dx: f32, dy: f32) -> u8 {
        let x = (dx * img.w as f32 / 64.0) as u32;
        let y = (dy * img.h as f32 / 64.0) as u32;
        img.data[((y * img.w + x) * 4 + 3) as usize]
    }

    #[test]
    fn the_buffer_is_width_times_height_times_four_for_every_size() {
        for size in [16, 22, 32, 44, 64] {
            let img = render(State::Connected, true, size, INK);
            assert_eq!((img.w, img.h), (size, size));
            assert_eq!(img.data.len(), (size * size * 4) as usize);
            assert!(
                img.data.chunks(4).any(|p| p[3] > 0),
                "{size}: something is drawn"
            );
        }
    }

    #[test]
    fn a_connected_icon_has_two_dots_and_the_line_between_them() {
        let img = render(State::Connected, false, 128, INK);
        for (x, y) in [(14.0, 32.0), (50.0, 32.0), (32.0, 32.0), (29.0, 32.0)] {
            assert_eq!(alpha_at(&img, x, y), 255, "({x},{y})");
        }
        assert_eq!(alpha_at(&img, 32.0, 14.0), 0, "nothing above the line");
    }

    #[test]
    fn a_waiting_icon_has_dots_but_no_line() {
        let img = render(State::Waiting, false, 128, INK);
        assert_eq!(alpha_at(&img, 14.0, 32.0), 255);
        assert_eq!(alpha_at(&img, 32.0, 32.0), 255, "the middle dot");
        // The gap between two dots is under a pixel wide: only the edge of a dot shows there.
        assert!(alpha_at(&img, 29.0, 32.0) <= 64, "a gap, not a line");
    }

    #[test]
    fn a_paused_icon_has_two_bars_between_the_dots() {
        let img = render(State::Paused, false, 128, INK);
        assert_eq!(alpha_at(&img, 28.75, 24.0), 255);
        assert_eq!(alpha_at(&img, 35.25, 40.0), 255);
        assert_eq!(alpha_at(&img, 32.0, 32.0), 0, "the gap between the bars");
        assert_eq!(alpha_at(&img, 28.75, 14.0), 0);
    }

    #[test]
    fn faded_states_are_fainter_than_connected() {
        let max = |s| {
            render(s, false, 64, INK)
                .data
                .chunks(4)
                .map(|p| p[3])
                .max()
                .unwrap()
        };
        assert_eq!(max(State::Connected), 255);
        assert!(max(State::NoDevices) < max(State::Connected));
        assert!(max(State::Stopped) < max(State::NoDevices));
        assert!(max(State::Stopped) > 0);
    }

    #[test]
    fn the_badge_is_a_dot_in_the_top_right_corner_only() {
        for state in ALL {
            let plain = render(state, false, 128, INK);
            let badged = render(state, true, 128, INK);
            assert_eq!(alpha_at(&plain, 52.0, 12.0), 0, "{state:?}");
            assert_eq!(alpha_at(&badged, 52.0, 12.0), 255, "{state:?}");
            assert_eq!(alpha_at(&badged, 4.0, 60.0), 0, "{state:?}");
        }
    }

    #[test]
    fn every_drawn_pixel_has_exactly_the_ink_colour_and_rendering_is_repeatable() {
        for state in ALL {
            let a = render(state, true, 44, INK);
            assert_eq!(a, render(state, true, 44, INK));
            for p in a.data.chunks(4).filter(|p| p[3] > 0) {
                assert_eq!(&p[..3], &INK, "{state:?}");
            }
        }
    }

    #[test]
    fn the_edges_are_smooth_not_jagged() {
        let img = render(State::Connected, false, 32, INK);
        let partial = img
            .data
            .chunks(4)
            .filter(|p| p[3] > 0 && p[3] < 255)
            .count();
        assert!(
            partial > 20,
            "anti-aliasing gives partly covered pixels, got {partial}"
        );
    }

    /// Writes target/tray-preview.png (every state, dark and light ink) to look at.
    #[test]
    #[ignore]
    fn preview_sheet() {
        let size = 64u32;
        let mut sheet = image::RgbaImage::new(size * 5 + 6 * 8, size * 4 + 5 * 8);
        for p in sheet.pixels_mut() {
            *p = image::Rgba([128, 128, 128, 255]);
        }
        for (row, (ink, bg, badge)) in [
            ([255u8, 255, 255], [38u8, 38, 38], false),
            ([255, 255, 255], [38, 38, 38], true),
            ([0, 0, 0], [235, 235, 235], false),
            ([0, 0, 0], [235, 235, 235], true),
        ]
        .into_iter()
        .enumerate()
        {
            for (col, state) in ALL.iter().enumerate() {
                let x0 = 8 + col as u32 * (size + 8);
                let y0 = 8 + row as u32 * (size + 8);
                let img = render(*state, badge, size, ink);
                for y in 0..size {
                    for x in 0..size {
                        let i = ((y * size + x) * 4) as usize;
                        let a = img.data[i + 3] as f32 / 255.0;
                        let px = [0, 1, 2]
                            .map(|c| (img.data[i + c] as f32 * a + bg[c] as f32 * (1.0 - a)) as u8);
                        sheet.put_pixel(x0 + x, y0 + y, image::Rgba([px[0], px[1], px[2], 255]));
                    }
                }
            }
        }
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tray-preview.png");
        sheet.save(path).unwrap();
    }
}
