//! PNG helpers (mod textures, screenshots).

use std::io::Cursor;

/// Decodes a PNG to RGBA8: `(width, height, pixels)`.
pub fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut dec = png::Decoder::new(Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let n = (info.width * info.height) as usize;
    let src = &buf[..info.buffer_size()];
    let mut out = Vec::with_capacity(n * 4);
    match info.color_type {
        png::ColorType::Rgba => out.extend_from_slice(src),
        png::ColorType::Rgb => src.chunks_exact(3).for_each(|p| out.extend_from_slice(&[p[0], p[1], p[2], 255])),
        png::ColorType::Grayscale => src.iter().for_each(|&g| out.extend_from_slice(&[g, g, g, 255])),
        png::ColorType::GrayscaleAlpha => src.chunks_exact(2).for_each(|p| out.extend_from_slice(&[p[0], p[0], p[0], p[1]])),
        png::ColorType::Indexed => return Err("indexed png was not expanded".into()),
    }
    Ok((info.width, info.height, out))
}

pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let data = encode_png(width, height, rgba).map_err(std::io::Error::other)?;
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let px: Vec<u8> = (0..4 * 3 * 4).map(|i| (i * 7) as u8).collect();
        let data = encode_png(4, 3, &px).unwrap();
        let (w, h, back) = decode_png(&data).unwrap();
        assert_eq!((w, h), (4, 3));
        assert_eq!(back, px);
    }
}
