//! Packed RGB to planar I420 (BT.601, limited range: what browsers assume
//! for an H.264 stream without colour information, and what FFmpeg's
//! `format=yuv420p` produced before), with area-average scaling.

/// Byte layout of a source pixel (QEMU/pixman little-endian formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packed {
    /// Bytes B, G, R, X.
    Bgrx,
    /// Bytes R, G, B, X.
    Rgbx,
    /// 16-bit little-endian r5g6b5.
    Rgb565,
}

impl Packed {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Packed::Bgrx | Packed::Rgbx => 4,
            Packed::Rgb565 => 2,
        }
    }

    #[inline(always)]
    fn rgb(self, p: &[u8]) -> (u32, u32, u32) {
        match self {
            Packed::Bgrx => (p[2] as u32, p[1] as u32, p[0] as u32),
            Packed::Rgbx => (p[0] as u32, p[1] as u32, p[2] as u32),
            Packed::Rgb565 => {
                let v = u16::from_le_bytes([p[0], p[1]]) as u32;
                let r = (v >> 11) & 0x1f;
                let g = (v >> 5) & 0x3f;
                let b = v & 0x1f;
                (
                    (r << 3) | (r >> 2),
                    (g << 2) | (g >> 4),
                    (b << 3) | (b >> 2),
                )
            }
        }
    }
}

/// A source picture: `width` x `height` pixels, rows `stride` bytes apart.
pub struct Source<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: Packed,
}

/// Destination planes (Y full size, U and V half size each way).
pub struct I420<'a> {
    pub y: &'a mut [u8],
    pub y_stride: usize,
    pub u: &'a mut [u8],
    pub u_stride: usize,
    pub v: &'a mut [u8],
    pub v_stride: usize,
    pub width: usize,
    pub height: usize,
}

#[inline(always)]
fn luma(r: u32, g: u32, b: u32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

/// Chroma from the sum of four pixels' components.
#[inline(always)]
fn chroma(r4: u32, g4: u32, b4: u32) -> (u8, u8) {
    let (r, g, b) = (r4 as i32, g4 as i32, b4 as i32);
    // components are sums of 4: divide by 4 inside the >> (10 instead of 8)
    let u = ((-38 * r - 74 * g + 112 * b + 512) >> 10) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 512) >> 10) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// Convert (and scale when the sizes differ) `src` into `dst`. Sizes of
/// `dst` must be even. Returns false when a buffer is too small.
pub fn convert(src: &Source, dst: &mut I420) -> bool {
    let (w, h) = (dst.width, dst.height);
    if w == 0 || h == 0 || w % 2 == 1 || h % 2 == 1 || src.width == 0 || src.height == 0 {
        return false;
    }
    let bpp = src.format.bytes_per_pixel();
    if src.stride < src.width * bpp
        || src.data.len() < src.stride * (src.height - 1) + src.width * bpp
    {
        return false;
    }
    if dst.y_stride < w
        || dst.u_stride < w / 2
        || dst.v_stride < w / 2
        || dst.y.len() < dst.y_stride * (h - 1) + w
        || dst.u.len() < dst.u_stride * (h / 2 - 1) + w / 2
        || dst.v.len() < dst.v_stride * (h / 2 - 1) + w / 2
    {
        return false;
    }
    if (src.width, src.height) == (w, h) {
        convert_same_size(src, dst);
    } else {
        let rgb = scale_rgb(src, w, h);
        let s = Source {
            data: &rgb,
            width: w,
            height: h,
            stride: w * 4,
            format: Packed::Rgbx,
        };
        convert_same_size(&s, dst);
    }
    true
}

fn convert_same_size(src: &Source, dst: &mut I420) {
    let (w, h) = (dst.width, dst.height);
    let bpp = src.format.bytes_per_pixel();
    let f = src.format;
    for cy in 0..h / 2 {
        let r0 = &src.data[(2 * cy) * src.stride..];
        let r1 = &src.data[(2 * cy + 1) * src.stride..];
        let (y0, y1) = {
            let (a, b) = dst.y.split_at_mut((2 * cy + 1) * dst.y_stride);
            (&mut a[2 * cy * dst.y_stride..], b)
        };
        let u = &mut dst.u[cy * dst.u_stride..];
        let v = &mut dst.v[cy * dst.v_stride..];
        for cx in 0..w / 2 {
            let o = 2 * cx * bpp;
            let (ra, ga, ba) = f.rgb(&r0[o..]);
            let (rb, gb, bb) = f.rgb(&r0[o + bpp..]);
            let (rc, gc, bc) = f.rgb(&r1[o..]);
            let (rd, gd, bd) = f.rgb(&r1[o + bpp..]);
            y0[2 * cx] = luma(ra, ga, ba);
            y0[2 * cx + 1] = luma(rb, gb, bb);
            y1[2 * cx] = luma(rc, gc, bc);
            y1[2 * cx + 1] = luma(rd, gd, bd);
            let (cu, cv) = chroma(ra + rb + rc + rd, ga + gb + gc + gd, ba + bb + bc + bd);
            u[cx] = cu;
            v[cx] = cv;
        }
    }
}

/// Area-average scale to `w` x `h`, output RGBX.
fn scale_rgb(src: &Source, w: usize, h: usize) -> Vec<u8> {
    let bpp = src.format.bytes_per_pixel();
    let span = |i: usize, n: usize, of: usize| {
        let a = i * of / n;
        let b = ((i + 1) * of / n).max(a + 1).min(of);
        (a, b)
    };
    let cols: Vec<(usize, usize)> = (0..w).map(|x| span(x, w, src.width)).collect();
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let (sy0, sy1) = span(y, h, src.height);
        let orow = &mut out[y * w * 4..(y + 1) * w * 4];
        for (x, &(sx0, sx1)) in cols.iter().enumerate() {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in sy0..sy1 {
                let row = &src.data[sy * src.stride..];
                for sx in sx0..sx1 {
                    let (pr, pg, pb) = src.format.rgb(&row[sx * bpp..]);
                    r += pr;
                    g += pg;
                    b += pb;
                }
            }
            let n = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            orow[x * 4] = ((r + n / 2) / n) as u8;
            orow[x * 4 + 1] = ((g + n / 2) / n) as u8;
            orow[x * 4 + 2] = ((b + n / 2) / n) as u8;
        }
    }
    out
}

/// Where a picture lands inside a window (letterboxed, aspect kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// The largest `sw` x `sh` rectangle that fits `dw` x `dh`, centred.
pub fn fit_rect(sw: usize, sh: usize, dw: usize, dh: usize) -> Rect {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return Rect::default();
    }
    let (w, h) = if dw * sh <= dh * sw {
        (dw, (sh * dw / sw).max(1))
    } else {
        ((sw * dh / sh).max(1), dh)
    };
    Rect {
        x: (dw - w) / 2,
        y: (dh - h) / 2,
        w,
        h,
    }
}

#[inline(always)]
fn clamp8(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

/// Planar 4:2:0 (BT.601 limited range) to 0RGB words (`0x00RRGGBB`, what
/// window surfaces take) scaled into `dst` (`dw` x `dh`, letterboxed in
/// black; nearest sample). Returns where the picture landed.
#[allow(clippy::too_many_arguments)]
pub fn i420_to_xrgb(
    y: &[u8],
    ys: usize,
    u: &[u8],
    us: usize,
    v: &[u8],
    vs: usize,
    sw: usize,
    sh: usize,
    dst: &mut [u32],
    dw: usize,
    dh: usize,
) -> Rect {
    let r = fit_rect(sw, sh, dw, dh);
    if dst.len() < dw * dh || r.w == 0 {
        return Rect::default();
    }
    dst[..dw * dh].fill(0);
    let xs: Vec<usize> = (0..r.w).map(|x| (x * sw / r.w).min(sw - 1)).collect();
    for oy in 0..r.h {
        let sy = (oy * sh / r.h).min(sh - 1);
        let yrow = &y[sy * ys..];
        let urow = &u[(sy / 2) * us..];
        let vrow = &v[(sy / 2) * vs..];
        let out = &mut dst[(r.y + oy) * dw + r.x..(r.y + oy) * dw + r.x + r.w];
        for (o, &sx) in out.iter_mut().zip(&xs) {
            let c = 298 * (yrow[sx] as i32 - 16);
            let d = urow[sx / 2] as i32 - 128;
            let e = vrow[sx / 2] as i32 - 128;
            let rr = clamp8((c + 409 * e + 128) >> 8);
            let gg = clamp8((c - 100 * d - 208 * e + 128) >> 8);
            let bb = clamp8((c + 516 * d + 128) >> 8);
            *o = (rr << 16) | (gg << 8) | bb;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn i420_round_trip_and_letterbox() {
        // red BGRX 4x2 -> I420 -> xrgb into an 8x8 window: 8x4 picture centred
        let data: Vec<u8> = [0u8, 0, 255, 0].iter().copied().cycle().take(32).collect();
        let s = Source {
            data: &data,
            width: 4,
            height: 2,
            stride: 16,
            format: Packed::Bgrx,
        };
        let (y, u, v) = run(&s, 4, 2);
        let mut out = vec![0xffff_ffffu32; 64];
        let r = i420_to_xrgb(&y, 4, &u, 2, &v, 2, 4, 2, &mut out, 8, 8);
        assert_eq!(
            r,
            Rect {
                x: 0,
                y: 2,
                w: 8,
                h: 4
            }
        );
        assert_eq!(out[0], 0, "letterbox is black");
        let px = out[3 * 8 + 3];
        let (rr, gg, bb) = (px >> 16, (px >> 8) & 0xff, px & 0xff);
        assert!(rr > 245 && gg < 10 && bb < 10, "{px:06x}");
        assert_eq!(
            fit_rect(1920, 1080, 1280, 1024),
            Rect {
                x: 0,
                y: 152,
                w: 1280,
                h: 720
            }
        );
        assert_eq!(fit_rect(0, 1, 2, 2), Rect::default());
    }

    use super::*;

    fn run(src: &Source, w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; w * h / 4];
        let mut v = vec![0u8; w * h / 4];
        let mut d = I420 {
            y: &mut y,
            y_stride: w,
            u: &mut u,
            u_stride: w / 2,
            v: &mut v,
            v_stride: w / 2,
            width: w,
            height: h,
        };
        assert!(convert(src, &mut d));
        (y, u, v)
    }

    #[test]
    fn primaries_match_bt601_limited() {
        // white, black, red, green, blue (BGRX)
        for (bgrx, want) in [
            ([255u8, 255, 255, 0], (235u8, 128u8, 128u8)),
            ([0, 0, 0, 0], (16, 128, 128)),
            ([0, 0, 255, 0], (82, 90, 240)),
            ([0, 255, 0, 0], (145, 54, 34)),
            ([255, 0, 0, 0], (41, 240, 110)),
        ] {
            let data: Vec<u8> = bgrx.iter().copied().cycle().take(16).collect();
            let s = Source {
                data: &data,
                width: 2,
                height: 2,
                stride: 8,
                format: Packed::Bgrx,
            };
            let (y, u, v) = run(&s, 2, 2);
            assert!(
                y.iter().all(|&p| (p as i32 - want.0 as i32).abs() <= 1),
                "{bgrx:?} y {y:?}"
            );
            assert!(
                (u[0] as i32 - want.1 as i32).abs() <= 1,
                "{bgrx:?} u {}",
                u[0]
            );
            assert!(
                (v[0] as i32 - want.2 as i32).abs() <= 1,
                "{bgrx:?} v {}",
                v[0]
            );
        }
    }

    #[test]
    fn formats_agree() {
        let bgrx = [10u8, 200, 30, 0];
        let rgbx = [30u8, 200, 10, 0];
        let a: Vec<u8> = bgrx.iter().copied().cycle().take(16).collect();
        let b: Vec<u8> = rgbx.iter().copied().cycle().take(16).collect();
        let sa = Source {
            data: &a,
            width: 2,
            height: 2,
            stride: 8,
            format: Packed::Bgrx,
        };
        let sb = Source {
            data: &b,
            width: 2,
            height: 2,
            stride: 8,
            format: Packed::Rgbx,
        };
        assert_eq!(run(&sa, 2, 2), run(&sb, 2, 2));
        // r5g6b5 full white
        let c = vec![0xffu8; 8];
        let sc = Source {
            data: &c,
            width: 2,
            height: 2,
            stride: 4,
            format: Packed::Rgb565,
        };
        assert_eq!(run(&sc, 2, 2).0, vec![235; 4]);
    }

    #[test]
    fn scaling_averages_areas() {
        // 4x2 source: left half black, right half white -> 2x2: black, white
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 0, 255, 255, 255, 0]);
        }
        let s = Source {
            data: &data,
            width: 4,
            height: 2,
            stride: 16,
            format: Packed::Bgrx,
        };
        let (y, _, _) = run(&s, 2, 2);
        assert_eq!(y, vec![16, 235, 16, 235]);
        // odd source size into an even one
        let odd = vec![128u8; 3 * 3 * 4];
        let s = Source {
            data: &odd,
            width: 3,
            height: 3,
            stride: 12,
            format: Packed::Bgrx,
        };
        let (y, _, _) = run(&s, 2, 2);
        assert!(y.iter().all(|&p| p == y[0]));
    }

    #[test]
    fn refuses_bad_buffers() {
        let data = vec![0u8; 15];
        let s = Source {
            data: &data,
            width: 2,
            height: 2,
            stride: 8,
            format: Packed::Bgrx,
        };
        let mut y = vec![0u8; 4];
        let mut u = vec![0u8; 1];
        let mut v = vec![0u8; 1];
        let mut d = I420 {
            y: &mut y,
            y_stride: 2,
            u: &mut u,
            u_stride: 1,
            v: &mut v,
            v_stride: 1,
            width: 2,
            height: 2,
        };
        assert!(!convert(&s, &mut d));
    }
}
