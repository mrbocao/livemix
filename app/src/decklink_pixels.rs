/// Copy BGRA with aspect-preserving letterboxing, preserving alpha on the keyed output.
pub(crate) fn blit(dst: &mut [u8], dw: usize, dh: usize, src: &[u8], sw: usize, sh: usize, keyed: bool) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 || dst.len() < dw * dh * 4 {
        return;
    }
    if (dw, dh) == (sw, sh) {
        dst[..dw * dh * 4].copy_from_slice(&src[..sw * sh * 4]);
        return;
    }
    let black = [0, 0, 0, if keyed { 0 } else { 255 }];
    for pixel in dst.chunks_exact_mut(4) { pixel.copy_from_slice(&black); }
    let scale = (dw as f64 / sw as f64).min(dh as f64 / sh as f64);
    let w = ((sw as f64 * scale) as usize).clamp(1, dw);
    let h = ((sh as f64 * scale) as usize).clamp(1, dh);
    let (ox, oy) = ((dw - w) / 2, (dh - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let from = (y * sh / h * sw + x * sw / w) * 4;
            let to = ((oy + y) * dw + ox + x) * 4;
            dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
}

#[test]
fn key_fill_preserves_alpha_and_clears_letterbox() {
    let mut output = [255; 12];
    blit(&mut output, 3, 1, &[12, 24, 36, 128], 1, 1, true);
    assert_eq!(output, [0, 0, 0, 0, 12, 24, 36, 128, 0, 0, 0, 0]);
}
