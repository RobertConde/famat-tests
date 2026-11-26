//! Images to PDF, losslessly: one page per image, the image filling it.
//!
//! - A JPEG is embedded exactly as it is (`DCTDecode`), as img2pdf does, so
//!   not a single bit changes.
//! - Any other format is decoded and its pixels stored Flate-compressed at
//!   their own depth (8 or 16 bits), with any transparency as a soft mask.
//!
//! The page is the image scaled to fit a US Letter page (turned for a
//! landscape image), since these are mostly scanned test pages.

use std::{io::Write, path::Path};

use flate2::{Compression, write::ZlibEncoder};
use image::ImageReader;
use lopdf::{Document, Object, Stream, dictionary};

/// US Letter, in points.
const PAGE_LONG: f32 = 792.0;
const PAGE_SHORT: f32 = 612.0;

/// Write a one-page PDF of the image at `input` to `output`.
pub fn image_to_pdf(input: &Path, output: &Path) -> Result<(), String> {
    let bytes = std::fs::read(input).map_err(|e| format!("reading the image: {e}"))?;
    let image = if bytes.starts_with(b"\xFF\xD8\xFF") {
        jpeg(bytes)?
    } else {
        decoded(input)?
    };
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let mask_id = image.mask.map(|mask| document.add_object(mask));
    let mut xobject = image.xobject;
    if let Some(mask_id) = mask_id {
        xobject.dict.set("SMask", mask_id);
    }
    let (width, height) = (image.width as f32, image.height as f32);
    let image_id = document.add_object(xobject);

    let (bound_w, bound_h) = if width > height {
        (PAGE_LONG, PAGE_SHORT)
    } else {
        (PAGE_SHORT, PAGE_LONG)
    };
    let scale = (bound_w / width).min(bound_h / height);
    let (page_w, page_h) = (width * scale, height * scale);
    let content = format!("q {page_w:.4} 0 0 {page_h:.4} 0 0 cm /Im0 Do Q");
    let content_id = document.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(page_w), Object::Real(page_h)],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog_id);
    document
        .save(output)
        .map(drop)
        .map_err(|e| format!("writing the PDF: {e}"))
}

struct Image {
    width: u32,
    height: u32,
    xobject: Stream,
    mask: Option<Stream>,
}

fn image_dict(width: u32, height: u32, color_space: &str, bits: i64) -> lopdf::Dictionary {
    dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => i64::from(width),
        "Height" => i64::from(height),
        "ColorSpace" => Object::Name(color_space.as_bytes().to_vec()),
        "BitsPerComponent" => bits,
    }
}

/// A JPEG, as it is: its size and colour model come from its frame header.
fn jpeg(bytes: Vec<u8>) -> Result<Image, String> {
    let frame = jpeg_frame(&bytes).ok_or("the JPEG has no readable frame header")?;
    if frame.precision != 8 {
        return Err(format!("{}-bit JPEGs can't be put in a PDF", frame.precision));
    }
    let color_space = match frame.components {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        4 => "DeviceCMYK",
        n => return Err(format!("a JPEG with {n} colour components can't be put in a PDF")),
    };
    let mut dict = image_dict(frame.width, frame.height, color_space, 8);
    dict.set("Filter", "DCTDecode");
    if frame.components == 4 && frame.adobe {
        // Adobe's CMYK JPEGs store inverted values.
        dict.set(
            "Decode",
            vec![
                1.into(),
                0.into(),
                1.into(),
                0.into(),
                1.into(),
                0.into(),
                1.into(),
                0.into(),
            ],
        );
    }
    Ok(Image {
        width: frame.width,
        height: frame.height,
        xobject: Stream::new(dict, bytes).with_compression(false),
        mask: None,
    })
}

struct Frame {
    width: u32,
    height: u32,
    precision: u8,
    components: u8,
    /// An Adobe APP14 marker, which means inverted CMYK.
    adobe: bool,
}

/// Walk a JPEG's markers up to its first frame header (SOFn).
fn jpeg_frame(bytes: &[u8]) -> Option<Frame> {
    let mut adobe = false;
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        // Fill bytes, and markers without a length.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if matches!(marker, 0x01 | 0xD0..=0xD8) {
            i += 2;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        let segment = bytes.get(i + 4..i + 2 + length)?;
        match marker {
            0xEE if segment.starts_with(b"Adobe") => adobe = true,
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let height = u16::from_be_bytes([*segment.get(1)?, *segment.get(2)?]);
                let width = u16::from_be_bytes([*segment.get(3)?, *segment.get(4)?]);
                return Some(Frame {
                    precision: *segment.first()?,
                    height: u32::from(height),
                    width: u32::from(width),
                    components: *segment.get(5)?,
                    adobe,
                });
            }
            0xDA => return None,
            _ => {}
        }
        i += 2 + length;
    }
    None
}

/// Any other image: decoded, then its pixels stored at their own depth.
fn decoded(input: &Path) -> Result<Image, String> {
    let image = ImageReader::open(input)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|e| format!("reading the image: {e}"))?
        .decode()
        .map_err(|e| format!("the image does not decode: {e}"))?;
    let (width, height) = (image.width(), image.height());
    let color = image.color();
    let gray = !color.has_color();
    let alpha = color.has_alpha();
    let sixteen = color.bytes_per_pixel() / color.channel_count() > 1;
    let channels = if gray { 1 } else { 3 };

    // Every sample, as big-endian bytes, colour channels then alpha.
    let samples: Vec<u16> = match (gray, alpha, sixteen) {
        (true, false, false) => widen(image.to_luma8().into_raw()),
        (true, true, false) => widen(image.to_luma_alpha8().into_raw()),
        (false, false, false) => widen(image.to_rgb8().into_raw()),
        (false, true, false) => widen(image.to_rgba8().into_raw()),
        (true, false, true) => image.to_luma16().into_raw(),
        (true, true, true) => image.to_luma_alpha16().into_raw(),
        (false, false, true) => image.to_rgb16().into_raw(),
        (false, true, true) => image.to_rgba16().into_raw(),
    };
    let per_pixel = channels + usize::from(alpha);
    let push = |out: &mut Vec<u8>, sample: u16| {
        if sixteen {
            out.extend_from_slice(&sample.to_be_bytes());
        } else {
            out.push(sample as u8);
        }
    };
    let mut color_bytes = Vec::with_capacity(samples.len() * if sixteen { 2 } else { 1 });
    let mut alpha_bytes = Vec::new();
    let opaque = if sixteen { u16::MAX } else { u16::from(u8::MAX) };
    let mut translucent = false;
    for pixel in samples.chunks_exact(per_pixel) {
        for &sample in &pixel[..channels] {
            push(&mut color_bytes, sample);
        }
        if alpha {
            translucent |= pixel[channels] != opaque;
            push(&mut alpha_bytes, pixel[channels]);
        }
    }
    let bits = if sixteen { 16 } else { 8 };
    let color_space = if gray { "DeviceGray" } else { "DeviceRGB" };
    let mask = if translucent {
        Some(flate(image_dict(width, height, "DeviceGray", bits), &alpha_bytes)?)
    } else {
        None
    };
    Ok(Image {
        width,
        height,
        xobject: flate(image_dict(width, height, color_space, bits), &color_bytes)?,
        mask,
    })
}

fn widen(samples: Vec<u8>) -> Vec<u16> {
    samples.into_iter().map(u16::from).collect()
}

fn flate(mut dict: lopdf::Dictionary, data: &[u8]) -> Result<Stream, String> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(data)
        .map_err(|e| format!("compressing the image: {e}"))?;
    let compressed = encoder.finish().map_err(|e| format!("compressing the image: {e}"))?;
    dict.set("Filter", "FlateDecode");
    Ok(Stream::new(dict, compressed).with_compression(false))
}
