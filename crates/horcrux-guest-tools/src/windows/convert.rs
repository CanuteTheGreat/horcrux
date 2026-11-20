//! BGRA -> NV12 (BT.709, limited range) on the CPU, with nearest-neighbour
//! scaling when the encode size differs from the capture size.

/// Convert `src` (tight BGRA, `sw`x`sh`) into `dst` (NV12, `dw`x`dh`, both
/// even). Rows are split over `threads` threads.
pub fn bgra_to_nv12(
    src: &[u8],
    sw: usize,
    sh: usize,
    dst: &mut Vec<u8>,
    dw: usize,
    dh: usize,
    threads: usize,
) {
    dst.resize(dw * dh * 3 / 2, 0);
    let (y_plane, uv_plane) = dst.split_at_mut(dw * dh);
    let xmap: Vec<usize> = (0..dw).map(|x| x * sw / dw).collect();
    let pairs = dh / 2;
    let threads = threads.clamp(1, pairs.max(1));
    let per = pairs.div_ceil(threads);
    std::thread::scope(|s| {
        let mut y_rest = y_plane;
        let mut uv_rest = uv_plane;
        for t in 0..threads {
            let first = t * per;
            let n = per.min(pairs.saturating_sub(first));
            if n == 0 {
                break;
            }
            let (y_chunk, yr) = y_rest.split_at_mut(n * 2 * dw);
            let (uv_chunk, uvr) = uv_rest.split_at_mut(n * dw);
            y_rest = yr;
            uv_rest = uvr;
            let xmap = &xmap;
            s.spawn(move || {
                for p in 0..n {
                    let dy0 = (first + p) * 2;
                    let r0 = &src[(dy0 * sh / dh) * sw * 4..][..sw * 4];
                    let r1 = &src[((dy0 + 1) * sh / dh) * sw * 4..][..sw * 4];
                    let (ya, yb) = y_chunk[p * 2 * dw..(p * 2 + 2) * dw].split_at_mut(dw);
                    let uv = &mut uv_chunk[p * dw..(p + 1) * dw];
                    for bx in 0..dw / 2 {
                        let mut sr = 0i32;
                        let mut sg = 0i32;
                        let mut sb = 0i32;
                        for (k, (row, yrow)) in
                            [(r0, &mut *ya), (r1, &mut *yb)].into_iter().enumerate()
                        {
                            let _ = k;
                            for dx in [bx * 2, bx * 2 + 1] {
                                let px = &row[xmap[dx] * 4..xmap[dx] * 4 + 3];
                                let (b, g, r) = (px[0] as i32, px[1] as i32, px[2] as i32);
                                yrow[dx] = (((47 * r + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                                sr += r;
                                sg += g;
                                sb += b;
                            }
                        }
                        let (r, g, b) = ((sr + 2) >> 2, (sg + 2) >> 2, (sb + 2) >> 2);
                        uv[bx * 2] =
                            (((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
                        uv[bx * 2 + 1] =
                            (((112 * r - 102 * g - 10 * b + 128) >> 8) + 128).clamp(16, 240) as u8;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_and_white() {
        let mut nv = Vec::new();
        let src: Vec<u8> = [0u8, 0, 0, 255].repeat(16);
        bgra_to_nv12(&src, 4, 4, &mut nv, 4, 4, 2);
        assert_eq!(nv[0], 16);
        assert_eq!(nv[16], 128);
        let src: Vec<u8> = [255u8, 255, 255, 255].repeat(16);
        bgra_to_nv12(&src, 4, 4, &mut nv, 2, 2, 1);
        assert_eq!(nv.len(), 6);
        assert!(nv[0] >= 234);
    }
}
