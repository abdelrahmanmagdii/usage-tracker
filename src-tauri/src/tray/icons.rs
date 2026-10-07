//! Menu-bar icon rasterization.

use super::*;

pub(super) fn provider_tray_icon(provider: Provider) -> Image<'static> {
    match provider {
        Provider::Codex => codex_tray_icon(),
        Provider::Claude => claude_tray_icon(),
        Provider::Cursor => cursor_tray_icon(),
        Provider::OpenCode => opencode_tray_icon(),
        Provider::Devin => devin_tray_icon(),
        Provider::Antigravity => antigravity_tray_icon(),
        Provider::Gemini => gemini_tray_icon(),
    }
}

/// Compact menu-bar mark for the tools currently in the title. One tool keeps
/// its logo; several tools become staggered colored bars in title order so the
/// percentages can be told apart the same way Codex purple and Claude coral
/// already are. An empty set falls back to the two-bar brand mark.
pub(super) fn combined_tray_icon(providers: &[Provider]) -> Image<'static> {
    match providers {
        [] => bars_tray_icon(&[Provider::Codex.color(), Provider::Claude.color()]),
        [only] => provider_tray_icon(*only),
        many => {
            let colors: Vec<[f64; 3]> = many.iter().map(|provider| provider.color()).collect();
            bars_tray_icon(&colors)
        }
    }
}

/// The app-wide brand mark: Codex purple + Claude coral bars. Also the compact
/// icon when those two tools are the ones showing.
pub fn usagebar_tray_icon() -> Image<'static> {
    combined_tray_icon(&[Provider::Codex, Provider::Claude])
}

pub(super) fn bar_slots(count: usize) -> Vec<(f64, f64, f64, f64)> {
    match count {
        0 | 1 => vec![(6.4, 13.6, 4.5, 15.0)],
        2 => vec![(4.0, 8.6, 4.5, 15.0), (11.4, 16.0, 8.0, 15.0)],
        3 => vec![
            (2.4, 6.8, 4.5, 15.0),
            (8.0, 12.4, 7.0, 15.0),
            (13.6, 18.0, 9.2, 15.0),
        ],
        5 => vec![
            (1.0, 4.0, 4.0, 15.0),
            (4.6, 7.6, 5.2, 15.0),
            (8.2, 11.2, 6.4, 15.0),
            (11.8, 14.8, 7.6, 15.0),
            (15.4, 18.4, 8.8, 15.0),
        ],
        6 => vec![
            (0.4, 3.2, 4.0, 15.0),
            (3.6, 6.4, 5.0, 15.0),
            (6.8, 9.6, 6.0, 15.0),
            (10.0, 12.8, 7.0, 15.0),
            (13.2, 16.0, 8.0, 15.0),
            (16.4, 19.2, 9.0, 15.0),
        ],
        _ => vec![
            (1.6, 5.0, 4.2, 15.0),
            (6.2, 9.6, 6.0, 15.0),
            (10.8, 14.2, 7.8, 15.0),
            (15.4, 18.8, 9.6, 15.0),
        ],
    }
}

pub(super) fn bars_tray_icon(colors: &[[f64; 3]]) -> Image<'static> {
    const WIDTH: u32 = 20;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let bars: Vec<(f64, f64, f64, f64, [f64; 3])> = bar_slots(colors.len())
        .into_iter()
        .zip(colors.iter().copied())
        .map(|(slot, color)| (slot.0, slot.1, slot.2, slot.3, color))
        .collect();
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            for &(x0, x1, top, bottom, color) in &bars {
                let mut coverage = 0_u32;
                for sample_y in 0..SAMPLES {
                    for sample_x in 0..SAMPLES {
                        let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                        let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                        let radius = (x1 - x0) / 2.0;
                        let cx = (x0 + x1) / 2.0;
                        let cy = py.clamp(top + radius, bottom - radius);
                        if (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius {
                            coverage += 1;
                        }
                    }
                }
                if coverage == 0 {
                    continue;
                }
                let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
                let index = ((y * WIDTH + x) * 4) as usize;
                if rgba[index + 3] == 0 {
                    rgba[index] = color[0] as u8;
                    rgba[index + 1] = color[1] as u8;
                    rgba[index + 2] = color[2] as u8;
                    rgba[index + 3] = alpha;
                }
            }
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

/// The Codex provider's own menu-bar mark for the extended (one-icon-per-tool)
/// layout: the cloud/terminal glyph in Codex purple. Compact layout uses
/// colored bars (`combined_tray_icon`) when more than one tool is showing.
pub fn codex_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut cloud_coverage = 0_u32;
            let mut terminal_coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_codex_cloud(px, py) {
                        cloud_coverage += 1;
                        if inside_terminal_glyph(px, py) {
                            terminal_coverage += 1;
                        }
                    }
                }
            }
            if cloud_coverage == 0 {
                continue;
            }
            let alpha = ((cloud_coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let glyph_mix = terminal_coverage as f64 / cloud_coverage as f64;
            let red = ((194.0 + (139.0 - 194.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let green = ((79.0 + (55.0 - 79.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let blue = ((255.0 + (235.0 - 255.0) * blend) * (1.0 - glyph_mix) + 255.0 * glyph_mix)
                .round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_codex_cloud(x: f64, y: f64) -> bool {
    [
        (6.2, 9.2, 4.0),
        (9.1, 6.0, 4.4),
        (13.6, 6.7, 4.1),
        (16.0, 9.7, 4.0),
        (13.2, 12.1, 4.3),
        (8.3, 12.0, 4.1),
    ]
    .into_iter()
    .any(|(cx, cy, radius)| (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius)
}

pub(super) fn inside_terminal_glyph(x: f64, y: f64) -> bool {
    let chevron = distance_to_segment(x, y, 6.8, 7.0, 8.6, 9.2) <= 0.72
        || distance_to_segment(x, y, 8.6, 9.2, 6.8, 11.5) <= 0.72;
    let underscore = distance_to_segment(x, y, 11.1, 11.2, 14.4, 11.2) <= 0.72;
    chevron || underscore
}

/// Claude's coral starburst for the extended (one-icon-per-tool) layout.
pub fn claude_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    const CENTER_X: f64 = 11.0;
    const CENTER_Y: f64 = 9.0;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    let rays: Vec<(f64, f64)> = (0..8)
        .map(|index| {
            let angle = std::f64::consts::FRAC_PI_4 * index as f64;
            // Cardinal rays reach a little farther than diagonals, echoing the
            // uneven spark of the Claude mark.
            let length = if index % 2 == 0 { 7.0 } else { 5.4 };
            (
                CENTER_X + angle.cos() * length,
                CENTER_Y + angle.sin() * length,
            )
        })
        .collect();

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    let inside = rays.iter().any(|(tip_x, tip_y)| {
                        distance_to_segment(px, py, CENTER_X, CENTER_Y, *tip_x, *tip_y) <= 0.78
                    });
                    if inside {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (217.0 + (191.0 - 217.0) * blend).round() as u8;
            let green = (119.0 + (94.0 - 119.0) * blend).round() as u8;
            let blue = (87.0 + (62.0 - 87.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

/// Cursor's teal pointer. Same canvas as Codex/Claude so the extended
/// icons sit at the same visual weight in the menu bar.
pub fn cursor_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_cursor_pointer(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (15.0 + (11.0 - 15.0) * blend).round() as u8;
            let green = (157.0 + (128.0 - 157.0) * blend).round() as u8;
            let blue = (142.0 + (116.0 - 142.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_cursor_pointer(x: f64, y: f64) -> bool {
    // Classic arrow cursor, tip at top-left, wing to the right, notch + tail
    // down the shaft — the Cursor app mark, sized for a 22×18 tray canvas.
    const VERTS: [(f64, f64); 7] = [
        (5.0, 2.2),
        (5.3, 15.5),
        (8.6, 12.1),
        (10.1, 16.6),
        (12.4, 15.6),
        (10.0, 11.4),
        (16.6, 10.1),
    ];
    point_in_polygon(x, y, &VERTS)
}

/// OpenCode's indigo O: a rounded rectangular ring, the square brand mark
/// compressed onto the same 22×18 canvas as the other tray logos.
pub fn opencode_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_opencode_mark(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (79.0 + (67.0 - 79.0) * blend).round() as u8;
            let green = (70.0 + (56.0 - 70.0) * blend).round() as u8;
            let blue = (229.0 + (202.0 - 229.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_opencode_mark(x: f64, y: f64) -> bool {
    inside_rounded_rect(x, y, 5.0, 2.2, 17.0, 15.8, 3.8)
        && !inside_rounded_rect(x, y, 8.6, 5.8, 13.4, 12.2, 1.6)
}

/// Devin's amber diamond for the extended (one-icon-per-tool) layout.
pub fn devin_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_devin_mark(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (212.0 + (186.0 - 212.0) * blend).round() as u8;
            let green = (132.0 + (104.0 - 132.0) * blend).round() as u8;
            let blue = (38.0 + (28.0 - 38.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_devin_mark(x: f64, y: f64) -> bool {
    const VERTS: [(f64, f64); 4] = [(11.0, 2.2), (17.8, 9.0), (11.0, 15.8), (4.2, 9.0)];
    point_in_polygon(x, y, &VERTS)
}

/// Antigravity's Gemini-blue four-point star for the extended layout.
pub fn antigravity_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_antigravity_mark(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (66.0 + (48.0 - 66.0) * blend).round() as u8;
            let green = (133.0 + (98.0 - 133.0) * blend).round() as u8;
            let blue = (244.0 + (210.0 - 244.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_antigravity_mark(x: f64, y: f64) -> bool {
    const VERTS: [(f64, f64); 8] = [
        (11.0, 1.6),
        (12.3, 7.7),
        (18.4, 9.0),
        (12.3, 10.3),
        (11.0, 16.4),
        (9.7, 10.3),
        (3.6, 9.0),
        (9.7, 7.7),
    ];
    point_in_polygon(x, y, &VERTS)
}

/// Gemini CLI's menu-bar mark for the extended layout: the Gemini sparkle in
/// magenta. Taller and pinched tighter than Antigravity's blue star so the two
/// Google-adjacent marks don't read as duplicates in the menu bar.
pub fn gemini_tray_icon() -> Image<'static> {
    const WIDTH: u32 = 22;
    const HEIGHT: u32 = 18;
    const SAMPLES: u32 = 4;
    let mut rgba = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut coverage = 0_u32;
            for sample_y in 0..SAMPLES {
                for sample_x in 0..SAMPLES {
                    let px = x as f64 + (sample_x as f64 + 0.5) / SAMPLES as f64;
                    let py = y as f64 + (sample_y as f64 + 0.5) / SAMPLES as f64;
                    if inside_gemini_mark(px, py) {
                        coverage += 1;
                    }
                }
            }
            if coverage == 0 {
                continue;
            }
            let alpha = ((coverage * 255) / (SAMPLES * SAMPLES)) as u8;
            let blend = y as f64 / (HEIGHT - 1) as f64;
            let red = (228.0 + (158.0 - 228.0) * blend).round() as u8;
            let green = (104.0 + (60.0 - 104.0) * blend).round() as u8;
            let blue = (216.0 + (182.0 - 216.0) * blend).round() as u8;
            let index = ((y * WIDTH + x) * 4) as usize;
            rgba[index..index + 4].copy_from_slice(&[red, green, blue, alpha]);
        }
    }
    Image::new_owned(rgba, WIDTH, HEIGHT)
}

pub(super) fn inside_gemini_mark(x: f64, y: f64) -> bool {
    const VERTS: [(f64, f64); 8] = [
        (11.0, 0.8),
        (12.8, 7.2),
        (19.2, 9.0),
        (12.8, 10.8),
        (11.0, 17.2),
        (9.2, 10.8),
        (2.8, 9.0),
        (9.2, 7.2),
    ];
    point_in_polygon(x, y, &VERTS)
}

pub(super) fn inside_rounded_rect(
    x: f64,
    y: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    radius: f64,
) -> bool {
    let cx = x.clamp(x0 + radius, x1 - radius);
    let cy = y.clamp(y0 + radius, y1 - radius);
    (x - cx).powi(2) + (y - cy).powi(2) <= radius * radius
}

pub(super) fn point_in_polygon(x: f64, y: f64, verts: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = verts.len() - 1;
    for i in 0..verts.len() {
        let (xi, yi) = verts[i];
        let (xj, yj) = verts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(super) fn distance_to_segment(x: f64, y: f64, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length_squared = dx * dx + dy * dy;
    let t = (((x - x0) * dx + (y - y0) * dy) / length_squared).clamp(0.0, 1.0);
    ((x - (x0 + t * dx)).powi(2) + (y - (y0 + t * dy)).powi(2)).sqrt()
}
