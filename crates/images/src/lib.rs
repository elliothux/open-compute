//! Bounded, in-process raster image inspection and transformation.
//!
//! The engine is deliberately data-only: it opens no path, performs no network
//! request, and owns no session or tenant authority. Callers supply verified,
//! bounded bytes and enforce wall-clock/concurrency policy around each call.

#![deny(missing_docs)]

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder as _, ImageEncoder as _, Rgba, RgbaImage};
use open_compute_core::{ErrorCode, ImagesConfig, PlatformError};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// Image engine contract revision.
pub const IMAGE_ENGINE_VERSION: u32 = 1;

/// Raster formats accepted or emitted by the Day1 engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RasterFormat {
    /// JPEG input and output.
    Jpeg,
    /// PNG input and output.
    Png,
    /// WebP input and lossless output.
    Webp,
    /// AVIF output. The selected pure-Rust stack does not decode AVIF.
    Avif,
}

impl RasterFormat {
    /// Stable MIME type returned by the Images facade.
    #[must_use]
    pub const fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
        }
    }

    const fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Png => image::ImageFormat::Png,
            Self::Webp => image::ImageFormat::WebP,
            Self::Avif => image::ImageFormat::Avif,
        }
    }
}

/// Fixed resize behavior supported by the Day1 Images binding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResizeFit {
    /// Do not enlarge an input that already fits.
    ScaleDown,
    /// Preserve aspect ratio inside the requested bounds.
    #[default]
    Contain,
    /// Fill the requested bounds and crop overflow.
    Cover,
    /// Alias for the fixed cover-and-crop behavior.
    Crop,
    /// Preserve aspect ratio and pad to the exact requested bounds.
    Pad,
}

/// Fixed crop/pad anchor supported by the Day1 engine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Gravity {
    /// Center horizontally and vertically.
    #[default]
    Center,
    /// Centered on the top edge.
    Top,
    /// Centered on the bottom edge.
    Bottom,
    /// Centered on the left edge.
    Left,
    /// Centered on the right edge.
    Right,
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
}

/// One canonical resize operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResizeOperation {
    /// Requested output width, if constrained.
    pub width: Option<u32>,
    /// Requested output height, if constrained.
    pub height: Option<u32>,
    /// Aspect-ratio and crop behavior.
    #[serde(default)]
    pub fit: ResizeFit,
    /// Crop or pad anchor.
    #[serde(default)]
    pub gravity: Gravity,
    /// RGBA pad color.
    #[serde(default)]
    pub background: [u8; 4],
}

/// One ordered image operation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ImageOperation {
    /// Resize/crop/pad the current image.
    Resize(ResizeOperation),
    /// Rotate clockwise by 90, 180, or 270 degrees.
    Rotate {
        /// Clockwise degrees.
        degrees: u16,
    },
    /// Flip the current image on one or both axes.
    Flip {
        /// Mirror left-to-right.
        horizontal: bool,
        /// Mirror top-to-bottom.
        vertical: bool,
    },
    /// Composite the current alpha channel over a solid color.
    Background {
        /// RGBA background color.
        rgba: [u8; 4],
    },
    /// Apply a bounded Gaussian blur.
    Blur {
        /// Blur sigma in the inclusive range 0.1 through 100.
        sigma: f32,
    },
    /// Draw one supplied overlay over the current image.
    Draw {
        /// Zero-based index in [`ImageJob::overlays`].
        overlay: u16,
        /// Horizontal destination coordinate.
        x: i32,
        /// Vertical destination coordinate.
        y: i32,
        /// Opacity in the inclusive range 0.0 through 1.0.
        opacity: f32,
    },
}

/// Requested output encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputOptions {
    /// Required output format.
    pub format: RasterFormat,
    /// Optional lossy quality. JPEG accepts 1 through 100; other Day1 codecs reject it.
    pub quality: Option<u8>,
    /// Animation is intentionally unsupported and must remain false.
    #[serde(default)]
    pub anim: bool,
}

/// Fully materialized, bounded transform input.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageJob {
    /// Base image bytes.
    pub input: Vec<u8>,
    /// Overlay image bytes, referenced by ordered draw operations.
    pub overlays: Vec<Vec<u8>>,
    /// Ordered transform and draw operations.
    pub operations: Vec<ImageOperation>,
    /// Required output encoding.
    pub output: OutputOptions,
}

/// Sanitized image metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    /// Detected raster format.
    pub format: RasterFormat,
    /// Exact input byte length.
    pub file_size: u64,
    /// Decoded width in pixels.
    pub width: u32,
    /// Decoded height in pixels.
    pub height: u32,
}

/// Encoded transform result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageOutput {
    /// Exact encoded bytes.
    pub bytes: Vec<u8>,
    /// Output format and MIME identity.
    pub format: RasterFormat,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
}

/// One deterministic native image engine configured with fixed resource limits.
#[derive(Clone, Debug)]
pub struct ImageEngine {
    limits: ImagesConfig,
}

impl ImageEngine {
    /// Construct an engine after validating its resource policy.
    #[must_use]
    pub const fn new(limits: ImagesConfig) -> Self {
        Self { limits }
    }

    /// Inspect one input without returning codec metadata or embedded profiles.
    pub fn info(&self, bytes: &[u8]) -> Result<ImageInfo, PlatformError> {
        let (format, width, height, _) = self.probe(bytes)?;
        Ok(ImageInfo {
            format,
            file_size: bytes.len() as u64,
            width,
            height,
        })
    }

    /// Decode, execute the ordered operation graph, and encode one bounded result.
    pub fn transform(&self, job: &ImageJob) -> Result<ImageOutput, PlatformError> {
        if job.operations.len() > usize::from(self.limits.max_operations)
            || job.overlays.len() > usize::from(self.limits.max_overlays)
            || job.output.anim
        {
            return Err(limit());
        }
        if job.output.quality.is_some() && job.output.format != RasterFormat::Jpeg {
            return Err(option());
        }
        let mut image = self.decode(&job.input)?;
        let mut decoded_overlays: Vec<Option<DynamicImage>> = vec![None; job.overlays.len()];
        for operation in &job.operations {
            image = match operation {
                ImageOperation::Resize(resize) => self.resize(image, resize)?,
                ImageOperation::Rotate { degrees } => match degrees {
                    90 => image.rotate90(),
                    180 => image.rotate180(),
                    270 => image.rotate270(),
                    _ => return Err(option()),
                },
                ImageOperation::Flip {
                    horizontal,
                    vertical,
                } => {
                    if !horizontal && !vertical {
                        return Err(option());
                    }
                    let image = if *horizontal { image.fliph() } else { image };
                    if *vertical { image.flipv() } else { image }
                }
                ImageOperation::Background { rgba } => flatten(&image, *rgba),
                ImageOperation::Blur { sigma } => {
                    if !sigma.is_finite() || !(0.1..=100.0).contains(sigma) {
                        return Err(option());
                    }
                    image.blur(*sigma)
                }
                ImageOperation::Draw {
                    overlay,
                    x,
                    y,
                    opacity,
                } => {
                    if !opacity.is_finite() || !(0.0..=1.0).contains(opacity) {
                        return Err(option());
                    }
                    let index = usize::from(*overlay);
                    let bytes = job.overlays.get(index).ok_or_else(option)?;
                    if decoded_overlays[index].is_none() {
                        decoded_overlays[index] = Some(self.decode(bytes)?);
                    }
                    let overlay = decoded_overlays[index].as_ref().ok_or_else(option)?;
                    draw(&image, overlay, *x, *y, *opacity)
                }
            };
            self.validate_dimensions(image.width(), image.height())?;
        }
        let width = image.width();
        let height = image.height();
        let bytes = encode(&image, job.output)?;
        if bytes.len() as u64 > self.limits.max_output_bytes {
            return Err(limit());
        }
        Ok(ImageOutput {
            bytes,
            format: job.output.format,
            width,
            height,
        })
    }

    fn probe(&self, bytes: &[u8]) -> Result<(RasterFormat, u32, u32, Orientation), PlatformError> {
        if bytes.is_empty() || bytes.len() as u64 > self.limits.max_input_bytes {
            return Err(limit());
        }
        let image_format = image::guess_format(bytes).map_err(|_| invalid_input())?;
        let format = supported_input(image_format)?;
        reject_animation(bytes, image_format)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(self.limits.max_dimension);
        limits.max_image_height = Some(self.limits.max_dimension);
        limits.max_alloc = Some(self.limits.max_pixels.saturating_mul(8));
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image_format);
        reader.limits(limits);
        let mut decoder = reader
            .into_decoder()
            .map_err(|error| decode_error(&error))?;
        let (width, height) = decoder.dimensions();
        if decoder.total_bytes() > self.limits.max_pixels.saturating_mul(8) {
            return Err(limit());
        }
        let orientation = decoder
            .orientation()
            .map_err(|error| decode_error(&error))?;
        let (width, height) = oriented_dimensions(width, height, orientation);
        self.validate_dimensions(width, height)?;
        Ok((format, width, height, orientation))
    }

    fn decode(&self, bytes: &[u8]) -> Result<DynamicImage, PlatformError> {
        let (format, width, height, orientation) = self.probe(bytes)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(self.limits.max_dimension);
        limits.max_image_height = Some(self.limits.max_dimension);
        limits.max_alloc = Some(self.limits.max_pixels.saturating_mul(8));
        let image_format = format.image_format();
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image_format);
        reader.limits(limits);
        let decoder = reader
            .into_decoder()
            .map_err(|error| decode_error(&error))?;
        let mut image =
            DynamicImage::from_decoder(decoder).map_err(|error| decode_error(&error))?;
        image.apply_orientation(orientation);
        if image.width() != width || image.height() != height {
            return Err(invalid_input());
        }
        Ok(DynamicImage::ImageRgba8(image.to_rgba8()))
    }

    fn validate_dimensions(&self, width: u32, height: u32) -> Result<(), PlatformError> {
        let pixels = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or_else(limit)?;
        if width == 0
            || height == 0
            || width > self.limits.max_dimension
            || height > self.limits.max_dimension
            || pixels > self.limits.max_pixels
        {
            return Err(limit());
        }
        Ok(())
    }

    fn resize(
        &self,
        image: DynamicImage,
        operation: &ResizeOperation,
    ) -> Result<DynamicImage, PlatformError> {
        if operation.width.is_none() && operation.height.is_none() {
            return Err(option());
        }
        let width = operation.width.unwrap_or_else(|| {
            scaled_dimension(image.width(), image.height(), operation.height.unwrap_or(1))
        });
        let height = operation.height.unwrap_or_else(|| {
            scaled_dimension(image.height(), image.width(), operation.width.unwrap_or(1))
        });
        self.validate_dimensions(width, height)?;
        match operation.fit {
            ResizeFit::ScaleDown if image.width() <= width && image.height() <= height => Ok(image),
            ResizeFit::ScaleDown | ResizeFit::Contain => {
                Ok(image.resize(width, height, FilterType::Lanczos3))
            }
            ResizeFit::Cover | ResizeFit::Crop => {
                Ok(resize_to_fill(&image, width, height, operation.gravity))
            }
            ResizeFit::Pad => {
                let resized = image.resize(width, height, FilterType::Lanczos3);
                let mut canvas = RgbaImage::from_pixel(width, height, Rgba(operation.background));
                let (x, y) = anchored_offset(
                    width,
                    height,
                    resized.width(),
                    resized.height(),
                    operation.gravity,
                );
                composite_source_over(
                    &mut canvas,
                    &resized.to_rgba8(),
                    i64::from(x),
                    i64::from(y),
                    1.0,
                );
                Ok(DynamicImage::ImageRgba8(canvas))
            }
        }
    }
}

fn oriented_dimensions(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    if matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    ) {
        (height, width)
    } else {
        (width, height)
    }
}

fn reject_animation(bytes: &[u8], format: image::ImageFormat) -> Result<(), PlatformError> {
    let animated = match format {
        image::ImageFormat::Png => png_has_animation(bytes),
        image::ImageFormat::WebP => webp_has_animation(bytes),
        _ => false,
    };
    if animated {
        Err(PlatformError::new(
            ErrorCode::ImageFormatUnsupported,
            "animated image input is not supported",
        ))
    } else {
        Ok(())
    }
}

fn png_has_animation(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return false;
    }
    let mut offset = 8_usize;
    while offset.checked_add(12).is_some_and(|end| end <= bytes.len()) {
        let Some(length) = bytes
            .get(offset..offset + 4)
            .and_then(|value| <[u8; 4]>::try_from(value).ok())
            .map(u32::from_be_bytes)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let kind = &bytes[offset + 4..offset + 8];
        if kind == b"acTL" {
            return true;
        }
        let Some(next) = offset
            .checked_add(12)
            .and_then(|value| value.checked_add(length))
        else {
            return false;
        };
        if next > bytes.len() || kind == b"IEND" {
            return false;
        }
        offset = next;
    }
    false
}

fn webp_has_animation(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return false;
    }
    let mut offset = 12_usize;
    while offset.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let kind = &bytes[offset..offset + 4];
        if matches!(kind, b"ANIM" | b"ANMF") {
            return true;
        }
        let Some(length) = bytes
            .get(offset + 4..offset + 8)
            .and_then(|value| <[u8; 4]>::try_from(value).ok())
            .map(u32::from_le_bytes)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let Some(next) = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(length))
            .and_then(|value| value.checked_add(length % 2))
        else {
            return false;
        };
        if next > bytes.len() {
            return false;
        }
        offset = next;
    }
    false
}

fn supported_input(format: image::ImageFormat) -> Result<RasterFormat, PlatformError> {
    match format {
        image::ImageFormat::Jpeg => Ok(RasterFormat::Jpeg),
        image::ImageFormat::Png => Ok(RasterFormat::Png),
        image::ImageFormat::WebP => Ok(RasterFormat::Webp),
        image::ImageFormat::Avif => Err(PlatformError::new(
            ErrorCode::ImageFormatUnsupported,
            "AVIF decoding is not supported by the selected native image engine",
        )),
        _ => Err(PlatformError::new(
            ErrorCode::ImageFormatUnsupported,
            "image format is not supported",
        )),
    }
}

fn scaled_dimension(primary: u32, secondary: u32, target_secondary: u32) -> u32 {
    if secondary == 0 {
        return 1;
    }
    u32::try_from(
        u64::from(primary)
            .saturating_mul(u64::from(target_secondary))
            .div_ceil(u64::from(secondary)),
    )
    .unwrap_or(u32::MAX)
    .max(1)
}

fn resize_to_fill(image: &DynamicImage, width: u32, height: u32, gravity: Gravity) -> DynamicImage {
    let scale_width = u64::from(width).saturating_mul(u64::from(image.height()));
    let scale_height = u64::from(height).saturating_mul(u64::from(image.width()));
    let (resized_width, resized_height) = if scale_width >= scale_height {
        (
            width,
            scaled_dimension(image.height(), image.width(), width),
        )
    } else {
        (
            scaled_dimension(image.width(), image.height(), height),
            height,
        )
    };
    let resized = image.resize_exact(resized_width, resized_height, FilterType::Lanczos3);
    let (x, y) = crop_offset(resized_width, resized_height, width, height, gravity);
    resized.crop_imm(x, y, width, height)
}

fn crop_offset(
    outer_w: u32,
    outer_h: u32,
    inner_w: u32,
    inner_h: u32,
    gravity: Gravity,
) -> (u32, u32) {
    let dx = outer_w.saturating_sub(inner_w);
    let dy = outer_h.saturating_sub(inner_h);
    let x = match gravity {
        Gravity::Left | Gravity::TopLeft | Gravity::BottomLeft => 0,
        Gravity::Right | Gravity::TopRight | Gravity::BottomRight => dx,
        Gravity::Center | Gravity::Top | Gravity::Bottom => dx / 2,
    };
    let y = match gravity {
        Gravity::Top | Gravity::TopLeft | Gravity::TopRight => 0,
        Gravity::Bottom | Gravity::BottomLeft | Gravity::BottomRight => dy,
        Gravity::Center | Gravity::Left | Gravity::Right => dy / 2,
    };
    (x, y)
}

fn anchored_offset(
    outer_w: u32,
    outer_h: u32,
    inner_w: u32,
    inner_h: u32,
    gravity: Gravity,
) -> (u32, u32) {
    crop_offset(outer_w, outer_h, inner_w, inner_h, gravity)
}

fn flatten(image: &DynamicImage, color: [u8; 4]) -> DynamicImage {
    let mut canvas = RgbaImage::from_pixel(image.width(), image.height(), Rgba(color));
    composite_source_over(&mut canvas, &image.to_rgba8(), 0, 0, 1.0);
    DynamicImage::ImageRgba8(canvas)
}

fn draw(base: &DynamicImage, overlay: &DynamicImage, x: i32, y: i32, opacity: f32) -> DynamicImage {
    let mut base = base.to_rgba8();
    let overlay = overlay.to_rgba8();
    composite_source_over(&mut base, &overlay, i64::from(x), i64::from(y), opacity);
    DynamicImage::ImageRgba8(base)
}

fn composite_source_over(base: &mut RgbaImage, overlay: &RgbaImage, x: i64, y: i64, opacity: f32) {
    for (source_x, source_y, source) in overlay.enumerate_pixels() {
        let destination_x = x + i64::from(source_x);
        let destination_y = y + i64::from(source_y);
        if destination_x < 0
            || destination_y < 0
            || destination_x >= i64::from(base.width())
            || destination_y >= i64::from(base.height())
        {
            continue;
        }
        let destination = base.get_pixel_mut(destination_x as u32, destination_y as u32);
        source_over(destination, *source, opacity);
    }
}

fn source_over(destination: &mut Rgba<u8>, source: Rgba<u8>, opacity: f32) {
    let source_alpha = (f32::from(source.0[3]) * opacity).round().clamp(0.0, 255.0) as u32;
    let destination_alpha = u32::from(destination.0[3]);
    let inverse_source = 255_u32.saturating_sub(source_alpha);
    let output_alpha_scaled = source_alpha
        .saturating_mul(255)
        .saturating_add(destination_alpha.saturating_mul(inverse_source));
    if output_alpha_scaled == 0 {
        *destination = Rgba([0, 0, 0, 0]);
        return;
    }
    for channel in 0..3 {
        let source_component = u32::from(source.0[channel]);
        let destination_component = u32::from(destination.0[channel]);
        let numerator = source_component
            .saturating_mul(source_alpha)
            .saturating_mul(255)
            .saturating_add(
                destination_component
                    .saturating_mul(destination_alpha)
                    .saturating_mul(inverse_source),
            );
        destination.0[channel] = u8::try_from(
            numerator
                .saturating_add(output_alpha_scaled / 2)
                .checked_div(output_alpha_scaled)
                .unwrap_or(0)
                .min(255),
        )
        .unwrap_or(255);
    }
    destination.0[3] = u8::try_from(
        output_alpha_scaled
            .saturating_add(127)
            .checked_div(255)
            .unwrap_or(0)
            .min(255),
    )
    .unwrap_or(255);
}

fn encode(image: &DynamicImage, options: OutputOptions) -> Result<Vec<u8>, PlatformError> {
    let mut bytes = Vec::new();
    if options.format == RasterFormat::Jpeg {
        let quality = options.quality.unwrap_or(85);
        if !(1..=100).contains(&quality) {
            return Err(option());
        }
        let rgb = image.to_rgb8();
        JpegEncoder::new_with_quality(&mut bytes, quality)
            .write_image(
                &rgb,
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|_| unavailable())?;
    } else {
        image
            .write_to(&mut Cursor::new(&mut bytes), options.format.image_format())
            .map_err(|_| unavailable())?;
    }
    Ok(bytes)
}

fn invalid_input() -> PlatformError {
    PlatformError::new(ErrorCode::ImageInputInvalid, "image input is invalid")
}

fn decode_error(error: &image::ImageError) -> PlatformError {
    match error {
        image::ImageError::Limits(_) => limit(),
        _ => invalid_input(),
    }
}

fn option() -> PlatformError {
    PlatformError::new(
        ErrorCode::ImageOptionUnsupported,
        "image transform option is unsupported or invalid",
    )
}

fn limit() -> PlatformError {
    PlatformError::new(
        ErrorCode::ImageLimitExceeded,
        "image execution limit was exceeded",
    )
}

fn unavailable() -> PlatformError {
    PlatformError::new(ErrorCode::ImageUnavailable, "native image encoding failed")
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
