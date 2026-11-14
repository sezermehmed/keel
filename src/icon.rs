//! The Keel mark, drawn into a PNG the menu entry can use.

const PLATE: [f32; 3] = [
    0x17 as f32 / 255.0,
    0x19 as f32 / 255.0,
    0x1f as f32 / 255.0,
];
const CREAM: [f32; 3] = [
    0xf4 as f32 / 255.0,
    0xf0 as f32 / 255.0,
    0xe8 as f32 / 255.0,
];
const COPPER: [f32; 3] = [
    0xe0 as f32 / 255.0,
    0x8a as f32 / 255.0,
    0x4f as f32 / 255.0,
];

pub fn rgba(size: u32) -> Vec<u8> {
    let samples = 3i32;
    let mut pixels = vec![0u8; size as usize * size as usize * 4];
    for y in 0..size {
        for x in 0..size {
            let mut accum = [0.0f32; 4];
            for sy in 0..samples {
                for sx in 0..samples {
                    let px = (x as f32 + (sx as f32 + 0.5) / samples as f32) * 128.0 / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / samples as f32) * 128.0 / size as f32;
                    let [r, g, b, a] = shade(px, py);
                    accum[0] += r * a;
                    accum[1] += g * a;
                    accum[2] += b * a;
                    accum[3] += a;
                }
            }
            let alpha = accum[3] / (samples * samples) as f32;
            if alpha <= 0.0 {
                continue;
            }
            let index = ((y * size + x) * 4) as usize;
            pixels[index] = ((accum[0] / accum[3]) * 255.0).round() as u8;
            pixels[index + 1] = ((accum[1] / accum[3]) * 255.0).round() as u8;
            pixels[index + 2] = ((accum[2] / accum[3]) * 255.0).round() as u8;
            pixels[index + 3] = (alpha * 255.0).round() as u8;
        }
    }
    pixels
}

pub fn png_bytes(size: u32) -> Vec<u8> {
    encode_png(size, size, &rgba(size))
}

fn shade(x: f32, y: f32) -> [f32; 4] {
    let distance = sd_rounded_box((x - 64.0, y - 64.0), (56.0, 56.0), 28.0);
    let coverage = (0.5 - distance).clamp(0.0, 1.0);
    if coverage <= 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    let rgb = if in_sail(x, y) {
        COPPER
    } else if in_mast(x, y) || in_hull(x, y) {
        CREAM
    } else {
        PLATE
    };
    [rgb[0], rgb[1], rgb[2], coverage]
}

fn in_mast(x: f32, y: f32) -> bool {
    sd_rounded_box((x - 64.0, y - 59.0), (5.0, 27.0), 2.0) <= 0.0
}

fn in_hull(x: f32, y: f32) -> bool {
    if !(34.0..=94.0).contains(&x) || y < 82.0 {
        return false;
    }
    let t = (x - 34.0) / 60.0;
    let curve = 82.0 + 60.0 * t * (1.0 - t);
    y <= curve
}

fn in_sail(x: f32, y: f32) -> bool {
    point_in_triangle((x, y), (70.0, 34.0), (104.0, 66.0), (70.0, 72.0))
}

fn point_in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let side = |u: (f32, f32), v: (f32, f32)| (p.0 - v.0) * (u.1 - v.1) - (u.0 - v.0) * (p.1 - v.1);
    let ab = side(a, b);
    let bc = side(b, c);
    let ca = side(c, a);
    (ab >= 0.0 && bc >= 0.0 && ca >= 0.0) || (ab <= 0.0 && bc <= 0.0 && ca <= 0.0)
}

fn sd_rounded_box(point: (f32, f32), half: (f32, f32), radius: f32) -> f32 {
    let qx = point.0.abs() - half.0 + radius;
    let qy = point.1.abs() - half.1 + radius;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    let stride = width as usize * 4;
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * stride..(y + 1) * stride]);
    }
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut png = Vec::with_capacity(raw.len() + 128);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    png.extend_from_slice(&chunk(b"IHDR", &ihdr));
    png.extend_from_slice(&chunk(b"IDAT", &zlib_store(&raw)));
    png.extend_from_slice(&chunk(b"IEND", &[]));
    png
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + data.len());
    body.extend_from_slice(kind);
    body.extend_from_slice(data);
    let crc = crc32(&body);
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    out.extend_from_slice(&[0x78, 0x01]);
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    } else {
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + 65535).min(data.len());
            let block = &data[offset..end];
            let last = end == data.len();
            out.push(if last { 1 } else { 0 });
            let len = block.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(block);
            offset = end;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in data {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn crc32_matches_the_png_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn icon_has_a_plate_a_mast_and_a_sail() {
        let pixels = rgba(128);
        assert_eq!(
            pixel(&pixels, 128, 0, 0)[3],
            0,
            "corner is outside the mark"
        );
        assert_eq!(pixel(&pixels, 128, 64, 20)[0..3], [0x17, 0x19, 0x1f]);
        assert_eq!(pixel(&pixels, 128, 64, 50)[0..3], [0xf4, 0xf0, 0xe8]);
        let sail = pixel(&pixels, 128, 86, 58);
        assert!(
            sail[0] > sail[2] && sail[0] > 180,
            "sail pixel {sail:?} should be copper"
        );

        let png = png_bytes(128);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let path = PathBuf::from("/tmp/keel-icon-preview.png");
        fs::write(&path, &png).unwrap();
        let decoded = decode_png_rgba(&png);
        assert_eq!(decoded.0, 128);
        assert_eq!(decoded.1, 128);
        assert_eq!(pixel(&decoded.2, 128, 64, 50)[0..3], [0xf4, 0xf0, 0xe8]);
    }

    fn pixel(pixels: &[u8], size: u32, x: u32, y: u32) -> &[u8] {
        let index = ((y * size + x) * 4) as usize;
        &pixels[index..index + 4]
    }

    fn decode_png_rgba(png: &[u8]) -> (u32, u32, Vec<u8>) {
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut offset = 8;
        let mut width = 0u32;
        let mut height = 0u32;
        let mut idat = Vec::new();
        while offset + 8 <= png.len() {
            let len = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data = &png[offset + 8..offset + 8 + len];
            if kind == b"IHDR" {
                width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                assert_eq!(data[8], 8);
                assert_eq!(data[9], 6);
            } else if kind == b"IDAT" {
                idat.extend_from_slice(data);
            }
            offset += 12 + len;
            if kind == b"IEND" {
                break;
            }
        }
        let inflated = inflate_stored(&idat);
        let stride = width as usize * 4;
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height as usize {
            let row = y * (stride + 1);
            assert_eq!(inflated[row], 0, "scanline filter");
            rgba.extend_from_slice(&inflated[row + 1..row + 1 + stride]);
        }
        (width, height, rgba)
    }

    fn inflate_stored(zlib: &[u8]) -> Vec<u8> {
        assert_eq!(&zlib[..2], &[0x78, 0x01]);
        let mut offset = 2;
        let mut out = Vec::new();
        loop {
            let header = zlib[offset];
            offset += 1;
            let len = u16::from_le_bytes(zlib[offset..offset + 2].try_into().unwrap()) as usize;
            let nlen = u16::from_le_bytes(zlib[offset + 2..offset + 4].try_into().unwrap());
            assert_eq!(nlen, !(len as u16));
            offset += 4;
            out.extend_from_slice(&zlib[offset..offset + len]);
            offset += len;
            if header & 1 == 1 {
                break;
            }
        }
        assert_eq!(
            u32::from_be_bytes(zlib[offset..offset + 4].try_into().unwrap()),
            adler32(&out)
        );
        out
    }
}
