//! How large an image shows: its pixels at the density its file records,
//! as Preview and Quick Look show it. A Retina screenshot records 144
//! dots per inch, so each of its pixels covers half a point and it shows
//! at half its pixel size, one pixel to a pixel on a Retina screen. An
//! image that records 72 dots per inch, or none, shows at a point a
//! pixel. Noura reads the density PNG keeps in its `pHYs` chunk and JPEG
//! keeps in its JFIF header or its Exif data.

/// The fewest and most dots per inch Noura believes a file. Anything
/// outside, such as the 1 dpi some tools write, shows at a point a pixel.
const DENSITY: std::ops::RangeInclusive<f32> = 18.0..=2304.0;

/// The density points are measured in: 72 to the inch.
const POINTS_PER_INCH: f32 = 72.0;

/// The points each pixel of the image in `bytes` covers, across and down:
/// 72 over the dots per inch its file records, or 1 when it records none
/// Noura reads.
pub fn points_per_pixel(bytes: &[u8]) -> [f32; 2] {
    let dpi = png_density(bytes).or_else(|| jpeg_density(bytes));
    match dpi {
        Some([x, y]) if DENSITY.contains(&x) && DENSITY.contains(&y) => {
            [POINTS_PER_INCH / x, POINTS_PER_INCH / y]
        }
        _ => [1.0, 1.0],
    }
}

fn u16_be(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(*bytes.get(at..)?.first_chunk()?))
}

fn u32_be(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// The dots per inch a PNG's `pHYs` chunk records, when it gives them in
/// pixels per meter. The chunk comes before the image data.
fn png_density(bytes: &[u8]) -> Option<[f32; 2]> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut rest = bytes.strip_prefix(SIGNATURE)?;
    loop {
        let length = usize::try_from(u32_be(rest, 0)?).ok()?;
        let kind = rest.get(4..8)?;
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        let data = rest.get(8..8usize.checked_add(length)?)?;
        // Unit 1 is the meter; 0 gives only the pixels' shape. No whole
        // number of pixels a meter makes 144 dpi, so a screenshot records
        // 5,669, which rounds to 144 as macOS reads it.
        if kind == b"pHYs" && data.len() == 9 && data[8] == 1 {
            let x = (u32_be(data, 0)? as f32 * 0.0254).round();
            let y = (u32_be(data, 4)? as f32 * 0.0254).round();
            return Some([x, y]);
        }
        // The chunk's data, then its 4-byte checksum.
        rest = rest.get(12usize.checked_add(length)?..)?;
    }
}

/// The dots per inch a JPEG records: in its JFIF header, or failing that
/// in its Exif data. Both come before the image data.
fn jpeg_density(bytes: &[u8]) -> Option<[f32; 2]> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    let mut exif = None;
    loop {
        if *bytes.get(at)? != 0xff {
            return exif;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            // Fill bytes before a marker.
            0xff => {
                at += 1;
                continue;
            }
            // Markers with no segment.
            0x01 | 0xd0..=0xd8 => {
                at += 2;
                continue;
            }
            // The image data, or its end.
            0xd9 | 0xda => return exif,
            _ => {}
        }
        let length = usize::from(u16_be(bytes, at + 2)?);
        let segment = bytes.get(at + 4..(at + 2).checked_add(length)?)?;
        if marker == 0xe0
            && let Some(jfif) = jfif_density(segment)
        {
            return Some(jfif);
        }
        if marker == 0xe1 && exif.is_none() {
            exif = segment.strip_prefix(b"Exif\0\0").and_then(tiff_density);
        }
        at += 2 + length;
    }
}

/// The dots per inch a JFIF header records, when it gives them per inch
/// or per centimeter.
fn jfif_density(segment: &[u8]) -> Option<[f32; 2]> {
    let header = segment.strip_prefix(b"JFIF\0")?;
    let per_inch = match *header.get(2)? {
        1 => 1.0,
        2 => 2.54,
        // 0 gives only the pixels' shape.
        _ => return None,
    };
    let x = f32::from(u16_be(header, 3)?) * per_inch;
    let y = f32::from(u16_be(header, 5)?) * per_inch;
    Some([x, y])
}

/// The dots per inch the first image directory of the TIFF data in a
/// JPEG's Exif segment records.
fn tiff_density(tiff: &[u8]) -> Option<[f32; 2]> {
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let bytes = *tiff.get(at..)?.first_chunk()?;
        Some(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let bytes = *tiff.get(at..)?.first_chunk()?;
        Some(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let directory = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(directory)?);
    let (mut x, mut y) = (None, None);
    // Inches unless the directory says otherwise.
    let mut per_inch = Some(1.0);
    for index in 0..entries {
        let entry = directory + 2 + index * 12;
        let tag = u16_at(entry)?;
        let kind = u16_at(entry + 2)?;
        // A RATIONAL is two LONGs, numerator then denominator, where the
        // entry's value points.
        let rational = || -> Option<f32> {
            if kind != 5 {
                return None;
            }
            let at = usize::try_from(u32_at(entry + 8)?).ok()?;
            let (numerator, denominator) = (u32_at(at)?, u32_at(at + 4)?);
            (denominator != 0).then(|| numerator as f32 / denominator as f32)
        };
        match tag {
            0x011a => x = rational(),
            0x011b => y = rational(),
            // A SHORT sits at the start of the entry's value.
            0x0128 if kind == 3 => {
                per_inch = match u16_at(entry + 8)? {
                    2 => Some(1.0),
                    3 => Some(2.54),
                    // 1 gives no unit.
                    _ => None,
                };
            }
            _ => {}
        }
    }
    let per_inch = per_inch?;
    Some([x? * per_inch, y? * per_inch])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG chunk: its length, its kind, its data and a checksum, which
    /// Noura doesn't check.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend_from_slice(kind);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&[0; 4]);
        chunk
    }

    /// A PNG's header and a `pHYs` chunk of `x` and `y` per `unit`.
    fn png(phys: Option<(u32, u32, u8)>) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(chunk(b"IHDR", &[0; 13]));
        if let Some((x, y, unit)) = phys {
            let mut data = x.to_be_bytes().to_vec();
            data.extend(y.to_be_bytes());
            data.push(unit);
            png.extend(chunk(b"pHYs", &data));
        }
        png.extend(chunk(b"IDAT", &[0; 4]));
        png
    }

    fn close(found: [f32; 2], expected: [f32; 2]) -> bool {
        (found[0] - expected[0]).abs() < 0.0001 && (found[1] - expected[1]).abs() < 0.0001
    }

    #[test]
    fn a_png_shows_at_the_density_its_phys_chunk_records() {
        // A macOS screenshot: 5,669 pixels a meter, 144 dpi.
        assert!(close(
            points_per_pixel(&png(Some((5669, 5669, 1)))),
            [0.5, 0.5]
        ));
        // 72 dpi, and 96 dpi as Windows writes.
        assert!(close(
            points_per_pixel(&png(Some((2835, 2835, 1)))),
            [1.0, 1.0]
        ));
        assert!(close(
            points_per_pixel(&png(Some((3780, 3780, 1)))),
            [0.75, 0.75]
        ));
        // Pixels twice as tall as wide.
        assert!(close(
            points_per_pixel(&png(Some((5669, 2835, 1)))),
            [0.5, 1.0]
        ));
    }

    #[test]
    fn an_image_with_no_density_or_an_implausible_one_shows_at_a_point_a_pixel() {
        for bytes in [
            png(None),
            // Unit 0: the pixels' shape, no density.
            png(Some((5669, 5669, 0))),
            // 1 dpi and a million.
            png(Some((39, 39, 1))),
            png(Some((39_370_079, 39_370_079, 1))),
            b"GIF89a".to_vec(),
            Vec::new(),
            // A PNG cut off inside its header.
            png(Some((5669, 5669, 1)))[..20].to_vec(),
        ] {
            assert_eq!(points_per_pixel(&bytes), [1.0, 1.0]);
        }
    }

    /// A JPEG's start, an APP0 JFIF segment of `jfif` if given, and an
    /// APP1 Exif segment of `exif` if given, then the image data.
    fn jpeg(jfif: Option<(u8, u16, u16)>, exif: Option<Vec<u8>>) -> Vec<u8> {
        let mut jpeg = vec![0xff, 0xd8];
        let mut segment = |marker: u8, data: &[u8]| {
            jpeg.extend([0xff, marker]);
            jpeg.extend(((data.len() + 2) as u16).to_be_bytes());
            jpeg.extend_from_slice(data);
        };
        if let Some((units, x, y)) = jfif {
            let mut data = b"JFIF\0\x01\x02".to_vec();
            data.push(units);
            data.extend(x.to_be_bytes());
            data.extend(y.to_be_bytes());
            data.extend([0, 0]);
            segment(0xe0, &data);
        }
        if let Some(exif) = exif {
            let mut data = b"Exif\0\0".to_vec();
            data.extend(exif);
            segment(0xe1, &data);
        }
        segment(0xdb, &[0; 4]);
        jpeg.extend([0xff, 0xda, 0, 2, 0xff, 0xd9]);
        jpeg
    }

    /// TIFF data whose first directory records `x` and `y` dots per
    /// `unit`, little-endian or big-endian.
    fn tiff(little: bool, x: u32, y: u32, unit: u16) -> Vec<u8> {
        let u16b = |value: u16| {
            if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let u32b = |value: u32| {
            if little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let mut tiff = if little {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        };
        tiff.extend(u16b(42));
        tiff.extend(u32b(8));
        // Three entries, then the rationals after the directory.
        let rationals = 8 + 2 + 3 * 12 + 4;
        tiff.extend(u16b(3));
        for (tag, at) in [(0x011a, rationals), (0x011b, rationals + 8)] {
            tiff.extend(u16b(tag));
            tiff.extend(u16b(5));
            tiff.extend(u32b(1));
            tiff.extend(u32b(at));
        }
        tiff.extend(u16b(0x0128));
        tiff.extend(u16b(3));
        tiff.extend(u32b(1));
        tiff.extend(u16b(unit));
        tiff.extend([0, 0]);
        tiff.extend(u32b(0));
        for value in [x, y] {
            tiff.extend(u32b(value));
            tiff.extend(u32b(1));
        }
        tiff
    }

    #[test]
    fn a_jpeg_shows_at_the_density_its_jfif_header_or_exif_data_records() {
        assert!(close(
            points_per_pixel(&jpeg(Some((1, 144, 144)), None)),
            [0.5, 0.5]
        ));
        // Dots per centimeter.
        assert!(close(
            points_per_pixel(&jpeg(Some((2, 118, 118)), None)),
            [72.0 / 299.72, 72.0 / 299.72]
        ));
        for little in [true, false] {
            assert!(close(
                points_per_pixel(&jpeg(None, Some(tiff(little, 144, 144, 2)))),
                [0.5, 0.5]
            ));
        }
        // A JFIF header that gives only the pixels' shape leaves Exif's.
        assert!(close(
            points_per_pixel(&jpeg(Some((0, 1, 1)), Some(tiff(false, 300, 300, 2)))),
            [0.24, 0.24]
        ));
        // Exif with no unit, and a JPEG with neither.
        assert_eq!(
            points_per_pixel(&jpeg(None, Some(tiff(true, 144, 144, 1)))),
            [1.0, 1.0]
        );
        assert_eq!(points_per_pixel(&jpeg(None, None)), [1.0, 1.0]);
    }

    #[test]
    fn a_malformed_header_shows_at_a_point_a_pixel() {
        let whole = jpeg(None, Some(tiff(true, 144, 144, 2)));
        for end in 0..whole.len() {
            let _ = points_per_pixel(&whole[..end]);
        }
        let mut wrong = tiff(true, 144, 144, 2);
        // The directory points past the end.
        wrong[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(points_per_pixel(&jpeg(None, Some(wrong))), [1.0, 1.0]);
        let mut png = png(Some((5669, 5669, 1)));
        // A chunk whose length runs past the end.
        png[33..37].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(points_per_pixel(&png), [1.0, 1.0]);
    }
}
