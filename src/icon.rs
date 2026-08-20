//! Programmatic tray icon: a rounded dark tile with two accent "quota bars",
//! echoing the strip itself. Returns 32x32 RGBA8.

pub const SIZE: u32 = 32;

pub fn rgba() -> Vec<u8> {
    let w = SIZE as i32;
    let h = SIZE as i32;
    let mut buf = vec![0u8; (w * h * 4) as usize];

    let bg = [26u8, 28, 34, 255];
    let track = [60u8, 64, 74, 255];
    let accent = [90u8, 150, 255, 255];
    let accent2 = [120u8, 200, 160, 255];

    let radius = 6i32;
    let put = |buf: &mut [u8], x: i32, y: i32, c: [u8; 4]| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let i = ((y * w + x) * 4) as usize;
        buf[i] = c[0];
        buf[i + 1] = c[1];
        buf[i + 2] = c[2];
        buf[i + 3] = c[3];
    };

    // Rounded background tile.
    for y in 0..h {
        for x in 0..w {
            let inside = rounded_inside(x, y, w, h, radius);
            if inside {
                put(&mut buf, x, y, bg);
            }
        }
    }

    // Two bars.
    let bar_h = 4i32;
    let left = 6i32;
    let right = w - 6;
    for (idx, (fill_frac, col)) in [(0.75f32, accent), (0.4f32, accent2)]
        .into_iter()
        .enumerate()
    {
        let y0 = 9 + idx as i32 * 9;
        let fill_x = left + ((right - left) as f32 * fill_frac) as i32;
        for y in y0..y0 + bar_h {
            for x in left..right {
                let c = if x <= fill_x { col } else { track };
                put(&mut buf, x, y, c);
            }
        }
    }

    buf
}

fn rounded_inside(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    let corners = [
        (r, r),
        (w - 1 - r, r),
        (r, h - 1 - r),
        (w - 1 - r, h - 1 - r),
    ];
    let in_x_band = x >= r && x <= w - 1 - r;
    let in_y_band = y >= r && y <= h - 1 - r;
    if in_x_band || in_y_band {
        return true;
    }
    // In a corner region: check distance to the matching corner center.
    let cx = if x < w / 2 {
        corners[0].0
    } else {
        corners[1].0
    };
    let cy = if y < h / 2 {
        corners[0].1
    } else {
        corners[2].1
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}
