use image::{Rgba, RgbaImage};

pub fn dense_text_source(width: u32, height: u32) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([248, 248, 246, 255]));

    for y in 0..height {
        for x in 0..width {
            let paper_noise = ((x.wrapping_mul(17) + y.wrapping_mul(13)) % 5) as u8;
            image.put_pixel(
                x,
                y,
                Rgba([248 - paper_noise, 248 - paper_noise, 246 - paper_noise, 255]),
            );
        }
    }

    // Varied glyph-like runs create a dense document without periodic aliases.
    let body_left = 34;
    let body_right = width.saturating_sub(28);
    for line in 0..(height / 14) {
        let baseline = 8 + line * 14;
        let mut x = body_left + ((line * 11) % 17);
        let mut word = 0u32;
        while x + 4 < body_right {
            let seed = line.wrapping_mul(97) ^ word.wrapping_mul(31);
            let word_width = 18 + seed % 46;
            let gap = 5 + seed % 6;
            let ink = 34 + (seed % 34) as u8;
            let end = (x + word_width).min(body_right);
            let mut glyph_x = x;

            while glyph_x < end {
                let glyph_seed = seed ^ glyph_x.wrapping_mul(7);
                let glyph_width = 1 + glyph_seed % 3;
                let glyph_end = (glyph_x + glyph_width).min(end);
                let glyph_top = baseline + (glyph_seed % 3);
                let glyph_bottom = (baseline + 8 + (glyph_seed % 3)).min(height);
                for fill_x in glyph_x..glyph_end {
                    for fill_y in glyph_top..glyph_bottom {
                        image.put_pixel(fill_x, fill_y, Rgba([ink, ink, ink, 255]));
                    }
                }
                glyph_x = glyph_end.saturating_add(1 + glyph_seed % 3);
            }

            x = end.saturating_add(gap);
            word = word.wrapping_add(1);
        }

        if line % 9 == 0 {
            let rule_y = baseline.saturating_add(10);
            if rule_y < height {
                for rule_x in body_left..body_right {
                    image.put_pixel(rule_x, rule_y, Rgba([112, 135, 166, 255]));
                }
            }
        }
    }

    image
}

pub fn dense_text_with_table_source(
    width: u32,
    height: u32,
    table_top: u32,
    table_height: u32,
) -> RgbaImage {
    let mut image = dense_text_source(width, height);
    draw_table(&mut image, table_top, table_height);
    image
}

pub fn rich_document_source(width: u32, height: u32) -> RgbaImage {
    let mut image = dense_text_source(width, height);
    let visual_left = (width / 10).max(12);
    let visual_width = width.saturating_sub(visual_left.saturating_mul(2));
    draw_visual_block(&mut image, visual_left, 660, visual_width, 560);
    draw_table(&mut image, 1_580, 1_460);
    image
}

fn draw_visual_block(image: &mut RgbaImage, left: u32, top: u32, width: u32, height: u32) {
    let right = left.saturating_add(width).min(image.width());
    let bottom = top.saturating_add(height).min(image.height());
    for y in top..bottom {
        for x in left..right {
            let local_x = x.saturating_sub(left);
            let local_y = y.saturating_sub(top);
            let texture = local_x
                .wrapping_mul(17)
                .wrapping_add(local_y.wrapping_mul(29))
                .wrapping_add(local_x.wrapping_mul(local_y) % 97);
            image.put_pixel(
                x,
                y,
                Rgba([
                    46 + (texture % 130) as u8,
                    54 + ((texture / 3) % 120) as u8,
                    68 + ((texture / 7) % 110) as u8,
                    255,
                ]),
            );
        }
    }

    for y in (top..bottom).step_by(67) {
        for x in left..right {
            image.put_pixel(x, y, Rgba([230, 238, 246, 255]));
        }
    }
}

fn draw_table(image: &mut RgbaImage, table_top: u32, table_height: u32) {
    let width = image.width();
    let height = image.height();
    let table_bottom = table_top.saturating_add(table_height).min(height);
    let left = (width / 32).max(8);
    let right = width.saturating_sub(left);
    let columns = [left, width / 2, right];
    let header_height = 56;
    let row_height = 72;

    for y in table_top..table_bottom {
        for x in left..right {
            let color = if y < table_top.saturating_add(header_height) {
                Rgba([232, 238, 248, 255])
            } else {
                Rgba([252, 252, 252, 255])
            };
            image.put_pixel(x, y, color);
        }
    }

    for x in columns {
        for y in table_top..table_bottom {
            image.put_pixel(x, y, Rgba([204, 212, 226, 255]));
        }
    }
    for y in (table_top..table_bottom).step_by(row_height as usize) {
        for x in left..right {
            image.put_pixel(x, y, Rgba([204, 212, 226, 255]));
        }
    }
    if table_bottom > table_top {
        for x in left..right {
            image.put_pixel(x, table_bottom - 1, Rgba([204, 212, 226, 255]));
        }
    }

    for (row_index, row_top) in (table_top..table_bottom)
        .step_by(row_height as usize)
        .enumerate()
    {
        let text_top = row_top.saturating_add(18);
        for (column_index, pair) in columns.windows(2).enumerate() {
            let text_left = pair[0].saturating_add(18);
            let text_right = pair[1].saturating_sub(18);
            let seed = (row_index as u32).wrapping_mul(31) ^ (column_index as u32 * 17);
            for y in text_top..(text_top + 20).min(table_bottom) {
                for x in text_left..text_right {
                    if (x + y + seed) % 7 < 4 && (x / 3 + seed) % 5 != 0 {
                        image.put_pixel(x, y, Rgba([68, 76, 92, 255]));
                    }
                }
            }
        }
    }
}

pub fn sparse_table_source(width: u32, height: u32) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([250, 250, 251, 255]));

    // A mostly empty grid: full-height vertical column borders and widely
    // spaced horizontal row borders, like a table of tall blank cells with a
    // single narrow text column.
    let border = Rgba([206, 210, 218, 255]);
    for x_frac in [0.03, 0.22, 0.41, 0.60, 0.78] {
        let x = (width as f32 * x_frac) as u32;
        for y in 0..height {
            image.put_pixel(x, y, border);
        }
    }
    for y in (0..height).step_by(400) {
        for x in 0..width {
            image.put_pixel(x, y, border);
        }
    }

    let text_left = (width as f32 * 0.84) as u32;
    let text_right = width.saturating_sub(24);
    for line in 0..(height / 16) {
        let baseline = 6 + line * 16;
        let mut x = text_left + ((line * 7) % 9);
        let mut word = 0u32;
        while x + 4 < text_right {
            let seed = line.wrapping_mul(97) ^ word.wrapping_mul(31);
            let word_width = 10 + seed % 22;
            let gap = 3 + seed % 5;
            let ink = 40 + (seed % 40) as u8;
            let end = (x + word_width).min(text_right);
            let mut glyph_x = x;

            while glyph_x < end {
                let glyph_seed = seed ^ glyph_x.wrapping_mul(7);
                let glyph_width = 1 + glyph_seed % 3;
                let glyph_end = (glyph_x + glyph_width).min(end);
                let glyph_top = baseline + (glyph_seed % 2);
                let glyph_bottom = (baseline + 7 + (glyph_seed % 2)).min(height);
                for fill_x in glyph_x..glyph_end {
                    for fill_y in glyph_top..glyph_bottom {
                        image.put_pixel(fill_x, fill_y, Rgba([ink, ink, ink, 255]));
                    }
                }
                glyph_x = glyph_end.saturating_add(1 + glyph_seed % 2);
            }

            x = end.saturating_add(gap);
            word = word.wrapping_add(1);
        }
    }

    image
}

pub fn crop(source: &RgbaImage, start_y: u32, height: u32) -> RgbaImage {
    image::imageops::crop_imm(source, 0, start_y, source.width(), height).to_image()
}

pub fn add_render_noise(image: &RgbaImage, amount: u8) -> RgbaImage {
    let mut noisy = image.clone();
    for (index, pixel) in noisy.pixels_mut().enumerate() {
        let delta = ((index as u32 * 37 + 11) % (amount as u32 * 2 + 1)) as i16 - amount as i16;
        for channel in &mut pixel.0[..3] {
            *channel = (*channel as i16 + delta).clamp(0, 255) as u8;
        }
    }
    noisy
}

pub fn add_animated_panel(image: &RgbaImage, left: u32, width: u32, value: u8) -> RgbaImage {
    let mut animated = image.clone();
    let right = (left + width).min(animated.width());
    for y in 0..animated.height() {
        for x in left..right {
            animated.put_pixel(x, y, Rgba([value, value.saturating_add(12), value, 255]));
        }
    }
    animated
}

pub fn add_animated_strip(image: &RgbaImage, top: u32, height: u32, value: u8) -> RgbaImage {
    let mut animated = image.clone();
    let bottom = (top + height).min(animated.height());
    for y in top..bottom {
        for x in 0..animated.width() {
            animated.put_pixel(x, y, Rgba([value, value.saturating_add(12), value, 255]));
        }
    }
    animated
}

pub fn with_static_sidebar(image: &RgbaImage, width: u32) -> RgbaImage {
    let mut frame = image.clone();
    let sidebar_width = width.min(frame.width());
    for y in 0..frame.height() {
        for x in 0..sidebar_width {
            let shade = if (x / 8 + y / 12) % 2 == 0 { 42 } else { 72 };
            frame.put_pixel(x, y, Rgba([shade, shade, shade, 255]));
        }
    }
    frame
}

pub fn dark_text_source(width: u32, height: u32) -> RgbaImage {
    let mut image = RgbaImage::from_pixel(width, height, Rgba([30, 32, 35, 255]));
    let body_left = 24;
    let body_right = width.saturating_sub(24);

    for line in 0..(height / 16) {
        let baseline = 4 + line * 16;
        let mut x = body_left + ((line * 13) % 23);
        let mut token = 0u32;
        while x + 3 < body_right {
            let seed = line.wrapping_mul(73) ^ token.wrapping_mul(19);
            let token_width = 8 + seed % 38;
            let token_end = (x + token_width).min(body_right);
            let shade = 150 + (seed % 80) as u8;
            for glyph_x in x..token_end {
                if (glyph_x + line) % 4 != 0 {
                    for glyph_y in baseline..(baseline + 9).min(height) {
                        image.put_pixel(glyph_x, glyph_y, Rgba([shade, shade, shade, 255]));
                    }
                }
            }
            x = token_end.saturating_add(5 + seed % 7);
            token = token.wrapping_add(1);
        }
    }

    image
}

pub fn fixed_header_frame(
    source: &RgbaImage,
    start_y: u32,
    frame_height: u32,
    header_height: u32,
) -> RgbaImage {
    let mut frame = RgbaImage::from_pixel(
        frame_width(source),
        frame_height,
        Rgba([248, 248, 246, 255]),
    );
    for y in 0..header_height.min(frame_height) {
        for x in 0..source.width() {
            let shade = 30 + ((x + y * 3) % 24) as u8;
            frame.put_pixel(x, y, Rgba([shade, 72, 128, 255]));
        }
    }
    let content_height = frame_height.saturating_sub(header_height);
    let content = crop(source, start_y, content_height);
    image::imageops::replace(&mut frame, &content, 0, header_height as i64);
    frame
}

fn frame_width(source: &RgbaImage) -> u32 {
    source.width()
}
