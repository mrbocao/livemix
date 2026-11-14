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
/// Convert 8-bit BGRA to limited-range Rec.709 v210 with pair-averaged chroma.
#[cfg(any(windows, test))]
pub(crate) fn pack_v210(dst: &mut [u8], stride: usize, bgra: &[u8], width: usize, height: usize) {
    fn yuv(pixel: &[u8]) -> (f32, f32, f32) {
        let (b, g, r) = (pixel[0] as f32, pixel[1] as f32, pixel[2] as f32);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        (64.0 + y * (876.0 / 255.0), 512.0 + (b - y) * (448.0 / (255.0 * 0.9278)),
            512.0 + (r - y) * (448.0 / (255.0 * 0.7874)))
    }
    fn pack(a: f32, b: f32, c: f32) -> [u8; 4] {
        let q = |value: f32| value.round().clamp(0.0, 1023.0) as u32;
        (q(a) | (q(b) << 10) | (q(c) << 20)).to_le_bytes()
    }
    if width == 0 || bgra.len() < width * height * 4 || dst.len() < stride * height { return; }
    for (line, output) in bgra.chunks_exact(width * 4).zip(dst.chunks_exact_mut(stride)).take(height) {
        for (block, words) in output.chunks_exact_mut(16).enumerate() {
            let pairs: [(f32, f32, f32, f32); 3] = std::array::from_fn(|pair| {
                let x = block * 6 + pair * 2;
                let read = |x: usize| if x < width { yuv(&line[x * 4..x * 4 + 4]) } else { (64.0, 512.0, 512.0) };
                let (y0, u0, v0) = read(x);
                let (y1, u1, v1) = read(x + 1);
                (y0, y1, (u0 + u1) * 0.5, (v0 + v1) * 0.5)
            });
            let [(y0, y1, u0, v0), (y2, y3, u1, v1), (y4, y5, u2, v2)] = pairs;
            for (slot, value) in words.chunks_exact_mut(4).zip([
                pack(u0, y0, v0), pack(y1, u1, y2), pack(v1, y3, u2), pack(y4, v2, y5),
            ]) { slot.copy_from_slice(&value); }
        }
    }
}

#[test]
fn v210_black_uses_legal_yuv_levels_and_padding() {
    let mut output = [0; 128];
    pack_v210(&mut output, 128, &[0; 24], 6, 1);
    let expected = (512u32 | 64 << 10 | 512 << 20).to_le_bytes();
    assert_eq!(&output[..4], &expected);
    assert_eq!(&output[16..20], &expected);
}
