use crate::ci_guard::Result;
use image::{ImageEncoder, Rgba, Rgba32FImage, RgbaImage, codecs::png::PngEncoder, imageops};
use std::{fs, path::Path};

const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
const PNG_SIZES: [(&str, u32); 14] = [
    ("32x32.png", 32),
    ("128x128.png", 128),
    ("128x128@2x.png", 256),
    ("icon.png", 512),
    ("StoreLogo.png", 50),
    ("Square30x30Logo.png", 30),
    ("Square44x44Logo.png", 44),
    ("Square71x71Logo.png", 71),
    ("Square89x89Logo.png", 89),
    ("Square107x107Logo.png", 107),
    ("Square142x142Logo.png", 142),
    ("Square150x150Logo.png", 150),
    ("Square284x284Logo.png", 284),
    ("Square310x310Logo.png", 310),
];

pub fn generate(output: &Path) -> Result<()> {
    let logo = circular_logo()?;
    fs::create_dir_all(output)?;
    for (name, size) in PNG_SIZES {
        fs::write(output.join(name), png(&logo, size)?)?;
    }
    let ico = encode_ico(&logo)?;
    validate_ico(&ico)?;
    fs::write(output.join("icon.ico"), ico)?;
    println!(
        "Generated approved PNG and ICO icons in {}",
        output.display()
    );
    Ok(())
}

fn circular_logo() -> Result<Rgba32FImage> {
    let source = image::open(crate::ci_guard::root().join("assets/logo.png"))?.into_rgb8();
    if source.dimensions() != (1254, 1254) {
        return Err("The approved logo must be 1254 × 1254 pixels".into());
    }
    Ok(Rgba32FImage::from_fn(1120, 1120, |x, y| {
        let [r, g, b] = source.get_pixel(x + 56, y + 46).0;
        // Sample at subpixel centers in eighth-pixel coordinates around the SVG circle.
        let mut covered = 0;
        for dy in [1, 3, 5, 7] {
            for dx in [1, 3, 5, 7] {
                let x = x as i32 * 8 + dx - 4480;
                let y = y as i32 * 8 + dy - 4480;
                covered += u8::from(x * x + y * y <= 4480 * 4480);
            }
        }
        let alpha = f32::from(covered) / 16.0;
        let premultiply = |c| f32::from(c) * alpha / 255.0;
        Rgba([premultiply(r), premultiply(g), premultiply(b), alpha])
    }))
}

fn png(logo: &Rgba32FImage, size: u32) -> Result<Vec<u8>> {
    // Lanczos expects premultiplied alpha; restore straight alpha for PNG.
    let scaled = imageops::resize(logo, size, size, imageops::FilterType::Lanczos3);
    let pixels = RgbaImage::from_fn(size, size, |x, y| {
        let [r, g, b, a] = scaled.get_pixel(x, y).0;
        let alpha = (a * 255.0).round() as u8;
        if alpha == 0 {
            Rgba([0; 4])
        } else {
            let rgb = |c: f32| ((c / a).clamp(0.0, 1.0) * 255.0).round() as u8;
            Rgba([rgb(r), rgb(g), rgb(b), alpha])
        }
    });
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(
        &pixels,
        size,
        size,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

fn directory_entry(size: u32) -> [u8; 8] {
    [size as u8, size as u8, 0, 0, 1, 0, 32, 0]
}

fn encode_ico(logo: &Rgba32FImage) -> Result<Vec<u8>> {
    let mut ico = b"\0\0\x01\0".to_vec();
    ico.extend_from_slice(&u16::try_from(ICO_SIZES.len())?.to_le_bytes());
    let mut offset = u32::try_from(6 + 16 * ICO_SIZES.len())?;
    let mut payloads = Vec::new();
    for size in ICO_SIZES {
        let payload = png(logo, size)?;
        let length = u32::try_from(payload.len())?;
        ico.extend_from_slice(&directory_entry(size));
        ico.extend_from_slice(&length.to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset = offset.checked_add(length).ok_or("ICO offset overflow")?;
        payloads.push(payload);
    }
    ico.extend(payloads.into_iter().flatten());
    Ok(ico)
}

pub(crate) fn validate_ico(bytes: &[u8]) -> Result<[u32; 7]> {
    let directory_end = 6 + 16 * ICO_SIZES.len();
    if bytes.len() < directory_end
        || bytes.len() > 8 * 1024 * 1024
        || !bytes.starts_with(b"\0\0\x01\0\x07\0")
    {
        return Err("Native icon container missing or malformed: icon.ico".into());
    }
    let mut offset = directory_end;
    for (entry, size) in bytes[6..directory_end].chunks_exact(16).zip(ICO_SIZES) {
        if entry[..8] != directory_entry(size)
            || usize::try_from(u32::from_le_bytes(entry[12..16].try_into()?))? != offset
        {
            return Err(format!("Invalid ICO directory entry for {size} px").into());
        }
        let length = usize::try_from(u32::from_le_bytes(entry[8..12].try_into()?))?;
        let end = offset
            .checked_add(length)
            .ok_or("ICO payload range overflow")?;
        let png = bytes.get(offset..end).ok_or("ICO payload is truncated")?;
        if png.len() < 33
            || !png.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
            || png[16..20] != size.to_be_bytes()
            || png[20..24] != size.to_be_bytes()
            || png[24..26] != [8, 6]
            || !png.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
        {
            return Err(format!("ICO entry is not a {size} px 32-bit RGBA PNG").into());
        }
        let pixels =
            image::load_from_memory_with_format(png, image::ImageFormat::Png)?.into_rgba8();
        check_alpha(&pixels, size)?;
        offset = end;
    }
    if offset != bytes.len() {
        return Err("Unexpected bytes after the final ICO entry".into());
    }
    Ok(ICO_SIZES)
}

pub(crate) fn check_alpha(pixels: &RgbaImage, size: u32) -> Result<()> {
    if pixels.dimensions() != (size, size)
        || [(0, 0), (0, size - 1), (size - 1, 0), (size - 1, size - 1)]
            .into_iter()
            .any(|(x, y)| pixels.get_pixel(x, y)[3] != 0)
        || pixels.get_pixel(size / 2, size / 2)[3] != 255
        || !pixels.pixels().any(|pixel| (1..255).contains(&pixel[3]))
    {
        return Err(format!("Native circular icon geometry/alpha failed at {size} px").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_icons_keep_circular_alpha_and_exact_png_frames() -> Result<()> {
        let output = tempfile::tempdir()?;
        generate(output.path())?;
        for (name, size) in PNG_SIZES {
            let pixels = image::open(output.path().join(name))?;
            assert_eq!(pixels.color(), image::ColorType::Rgba8);
            check_alpha(&pixels.into_rgba8(), size)?;
        }
        let ico = fs::read(output.path().join("icon.ico"))?;
        assert_eq!(&ico[..6], b"\0\0\x01\0\x07\0");
        assert_eq!(validate_ico(&ico)?, [16, 24, 32, 48, 64, 128, 256]);
        Ok(())
    }

    #[test]
    fn ico_validation_rejects_missing_and_corrupt_frames() -> Result<()> {
        let icon = circular_logo()?;
        let mut ico = encode_ico(&icon)?;
        assert!(validate_ico(&ico[..ico.len() - 1]).is_err());
        ico[6] = 20;
        assert!(validate_ico(&ico).is_err());
        ico[6] = 16;
        let start = usize::try_from(u32::from_le_bytes(ico[18..22].try_into()?))?;
        ico[start] ^= 1;
        assert!(validate_ico(&ico).is_err());
        Ok(())
    }
}
