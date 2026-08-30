use super::*;

fn fixture(format: image::ImageFormat) -> Vec<u8> {
    let mut image = RgbaImage::new(4, 3);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = Rgba([
            (x * 50) as u8,
            (y * 70) as u8,
            100,
            if x == 0 { 128 } else { 255 },
        ]);
    }
    let mut bytes = Vec::new();
    DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut bytes), format)
        .unwrap();
    bytes
}

fn job(operations: Vec<ImageOperation>) -> ImageJob {
    ImageJob {
        input: fixture(image::ImageFormat::Png),
        overlays: Vec::new(),
        operations,
        output: OutputOptions {
            format: RasterFormat::Png,
            quality: None,
            anim: false,
        },
    }
}

fn jpeg_with_orientation(orientation: u16) -> Vec<u8> {
    let jpeg = fixture(image::ImageFormat::Jpeg);
    let mut exif = Vec::from(b"Exif\0\0MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".as_slice());
    exif.extend_from_slice(&orientation.to_be_bytes());
    exif.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut bytes = Vec::with_capacity(jpeg.len() + exif.len() + 4);
    bytes.extend_from_slice(&jpeg[..2]);
    bytes.extend_from_slice(&[0xff, 0xe1]);
    bytes.extend_from_slice(&u16::try_from(exif.len() + 2).unwrap().to_be_bytes());
    bytes.extend_from_slice(&exif);
    bytes.extend_from_slice(&jpeg[2..]);
    bytes
}

#[test]
fn info_restricts_formats_bytes_dimensions_and_pixels() {
    let engine = ImageEngine::new(ImagesConfig::default());
    for (format, expected) in [
        (image::ImageFormat::Jpeg, RasterFormat::Jpeg),
        (image::ImageFormat::Png, RasterFormat::Png),
        (image::ImageFormat::WebP, RasterFormat::Webp),
    ] {
        let bytes = fixture(format);
        assert_eq!(
            engine.info(&bytes).unwrap(),
            ImageInfo {
                format: expected,
                file_size: bytes.len() as u64,
                width: 4,
                height: 3
            }
        );
    }
    assert_eq!(
        engine.info(b"not-an-image").unwrap_err().code(),
        ErrorCode::ImageInputInvalid
    );
    let limited = ImagesConfig {
        max_input_bytes: 4,
        ..ImagesConfig::default()
    };
    assert_eq!(
        ImageEngine::new(limited)
            .info(&fixture(image::ImageFormat::Png))
            .unwrap_err()
            .code(),
        ErrorCode::ImageLimitExceeded
    );
    for limited in [
        ImagesConfig {
            max_pixels: 11,
            ..ImagesConfig::default()
        },
        ImagesConfig {
            max_dimension: 3,
            ..ImagesConfig::default()
        },
    ] {
        assert_eq!(
            ImageEngine::new(limited)
                .info(&fixture(image::ImageFormat::Png))
                .unwrap_err()
                .code(),
            ErrorCode::ImageLimitExceeded
        );
    }
}

#[test]
fn exif_orientation_is_applied_and_animated_inputs_fail_closed() {
    let engine = ImageEngine::new(ImagesConfig::default());
    let oriented = jpeg_with_orientation(6);
    assert_eq!(
        (
            engine.info(&oriented).unwrap().width,
            engine.info(&oriented).unwrap().height
        ),
        (3, 4)
    );
    let output = engine
        .transform(&ImageJob {
            input: oriented,
            overlays: Vec::new(),
            operations: Vec::new(),
            output: OutputOptions {
                format: RasterFormat::Png,
                quality: None,
                anim: false,
            },
        })
        .unwrap();
    assert_eq!((output.width, output.height), (3, 4));

    let mut animated_png = fixture(image::ImageFormat::Png);
    animated_png.splice(
        8..8,
        [
            0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    let mut animated_webp = fixture(image::ImageFormat::WebP);
    animated_webp.splice(12..12, [b'A', b'N', b'I', b'M', 0, 0, 0, 0]);
    for input in [animated_png, animated_webp] {
        assert_eq!(
            engine.info(&input).unwrap_err().code(),
            ErrorCode::ImageFormatUnsupported
        );
    }
}

#[test]
fn ordered_resize_flip_background_blur_and_draw_are_bounded() {
    let engine = ImageEngine::new(ImagesConfig::default());
    let output = engine
        .transform(&ImageJob {
            input: fixture(image::ImageFormat::Png),
            overlays: vec![fixture(image::ImageFormat::Png)],
            operations: vec![
                ImageOperation::Resize(ResizeOperation {
                    width: Some(8),
                    height: Some(8),
                    fit: ResizeFit::Pad,
                    gravity: Gravity::BottomRight,
                    background: [10, 20, 30, 255],
                }),
                ImageOperation::Flip {
                    horizontal: true,
                    vertical: false,
                },
                ImageOperation::Rotate { degrees: 90 },
                ImageOperation::Blur { sigma: 0.5 },
                ImageOperation::Draw {
                    overlay: 0,
                    x: 1,
                    y: 1,
                    opacity: 0.5,
                },
            ],
            output: OutputOptions {
                format: RasterFormat::Png,
                quality: None,
                anim: false,
            },
        })
        .unwrap();
    assert_eq!(
        (output.width, output.height, output.format),
        (8, 8, RasterFormat::Png)
    );
    assert_eq!(
        engine.info(&output.bytes).unwrap().format,
        RasterFormat::Png
    );
}

#[test]
fn output_matrix_and_unknown_options_fail_closed() {
    let engine = ImageEngine::new(ImagesConfig::default());
    for format in [
        RasterFormat::Jpeg,
        RasterFormat::Png,
        RasterFormat::Webp,
        RasterFormat::Avif,
    ] {
        let result = engine
            .transform(&ImageJob {
                input: fixture(image::ImageFormat::Png),
                overlays: Vec::new(),
                operations: Vec::new(),
                output: OutputOptions {
                    format,
                    quality: (format == RasterFormat::Jpeg).then_some(90),
                    anim: false,
                },
            })
            .unwrap();
        assert_eq!(result.format, format);
        assert!(!result.bytes.is_empty());
    }
    let error = engine
        .transform(&ImageJob {
            input: fixture(image::ImageFormat::Png),
            overlays: Vec::new(),
            operations: vec![ImageOperation::Rotate { degrees: 45 }],
            output: OutputOptions {
                format: RasterFormat::Png,
                quality: None,
                anim: false,
            },
        })
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ImageOptionUnsupported);

    let limited = ImageEngine::new(ImagesConfig {
        max_output_bytes: 1,
        ..ImagesConfig::default()
    });
    assert_eq!(
        limited
            .transform(&ImageJob {
                input: fixture(image::ImageFormat::Png),
                overlays: Vec::new(),
                operations: Vec::new(),
                output: OutputOptions {
                    format: RasterFormat::Png,
                    quality: None,
                    anim: false,
                },
            })
            .unwrap_err()
            .code(),
        ErrorCode::ImageLimitExceeded
    );
}

#[test]
fn pad_gravity_and_draw_opacity_have_pixel_exact_ordered_results() {
    let engine = ImageEngine::new(ImagesConfig::default());
    let mut base = RgbaImage::new(2, 1);
    base.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    base.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
    let mut base_bytes = Vec::new();
    DynamicImage::ImageRgba8(base)
        .write_to(&mut Cursor::new(&mut base_bytes), image::ImageFormat::Png)
        .unwrap();
    let overlay = DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([0, 255, 0, 255])));
    let mut overlay_bytes = Vec::new();
    overlay
        .write_to(
            &mut Cursor::new(&mut overlay_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    let output = engine
        .transform(&ImageJob {
            input: base_bytes,
            overlays: vec![overlay_bytes],
            operations: vec![
                ImageOperation::Resize(ResizeOperation {
                    width: Some(4),
                    height: Some(4),
                    fit: ResizeFit::Pad,
                    gravity: Gravity::BottomRight,
                    background: [10, 20, 30, 255],
                }),
                ImageOperation::Draw {
                    overlay: 0,
                    x: 0,
                    y: 0,
                    opacity: 0.5,
                },
            ],
            output: OutputOptions {
                format: RasterFormat::Png,
                quality: None,
                anim: false,
            },
        })
        .unwrap();
    let pixels = image::load_from_memory_with_format(&output.bytes, image::ImageFormat::Png)
        .unwrap()
        .to_rgba8();
    assert_eq!(pixels.get_pixel(3, 3), &Rgba([0, 0, 255, 255]));
    assert_eq!(pixels.get_pixel(0, 1), &Rgba([10, 20, 30, 255]));
    assert_eq!(pixels.get_pixel(0, 0), &Rgba([5, 138, 15, 255]));
}

#[test]
fn every_operation_variant_and_option_limit_is_fail_closed() {
    let engine = ImageEngine::new(ImagesConfig::default());
    let output = engine
        .transform(&job(vec![
            ImageOperation::Resize(ResizeOperation {
                width: Some(2),
                height: Some(2),
                fit: ResizeFit::ScaleDown,
                gravity: Gravity::Center,
                background: [0; 4],
            }),
            ImageOperation::Resize(ResizeOperation {
                width: Some(6),
                height: Some(4),
                fit: ResizeFit::Contain,
                gravity: Gravity::Center,
                background: [0; 4],
            }),
            ImageOperation::Resize(ResizeOperation {
                width: Some(3),
                height: Some(5),
                fit: ResizeFit::Cover,
                gravity: Gravity::TopLeft,
                background: [0; 4],
            }),
            ImageOperation::Resize(ResizeOperation {
                width: Some(4),
                height: Some(3),
                fit: ResizeFit::Crop,
                gravity: Gravity::Right,
                background: [0; 4],
            }),
            ImageOperation::Resize(ResizeOperation {
                width: None,
                height: Some(2),
                fit: ResizeFit::Contain,
                gravity: Gravity::Center,
                background: [0; 4],
            }),
            ImageOperation::Resize(ResizeOperation {
                width: Some(2),
                height: None,
                fit: ResizeFit::Contain,
                gravity: Gravity::Center,
                background: [0; 4],
            }),
            ImageOperation::Rotate { degrees: 180 },
            ImageOperation::Rotate { degrees: 270 },
            ImageOperation::Flip {
                horizontal: false,
                vertical: true,
            },
            ImageOperation::Flip {
                horizontal: true,
                vertical: true,
            },
            ImageOperation::Background {
                rgba: [1, 2, 3, 255],
            },
        ]))
        .unwrap();
    assert_eq!((output.width, output.height), (1, 2));

    let invalid_operations = [
        ImageOperation::Resize(ResizeOperation {
            width: None,
            height: None,
            fit: ResizeFit::Contain,
            gravity: Gravity::Center,
            background: [0; 4],
        }),
        ImageOperation::Flip {
            horizontal: false,
            vertical: false,
        },
        ImageOperation::Blur { sigma: f32::NAN },
        ImageOperation::Blur { sigma: 101.0 },
        ImageOperation::Draw {
            overlay: 0,
            x: 0,
            y: 0,
            opacity: -0.1,
        },
        ImageOperation::Draw {
            overlay: 0,
            x: 0,
            y: 0,
            opacity: 1.1,
        },
        ImageOperation::Draw {
            overlay: 0,
            x: 0,
            y: 0,
            opacity: 1.0,
        },
    ];
    for operation in invalid_operations {
        assert_eq!(
            engine.transform(&job(vec![operation])).unwrap_err().code(),
            ErrorCode::ImageOptionUnsupported
        );
    }

    for limits in [
        ImagesConfig {
            max_operations: 0,
            ..ImagesConfig::default()
        },
        ImagesConfig {
            max_overlays: 0,
            ..ImagesConfig::default()
        },
    ] {
        let mut limited_job = job(vec![ImageOperation::Rotate { degrees: 90 }]);
        if limits.max_overlays == 0 {
            limited_job.operations.clear();
            limited_job.overlays.push(fixture(image::ImageFormat::Png));
        }
        assert_eq!(
            ImageEngine::new(limits)
                .transform(&limited_job)
                .unwrap_err()
                .code(),
            ErrorCode::ImageLimitExceeded
        );
    }

    let mut animated = job(Vec::new());
    animated.output.anim = true;
    assert_eq!(
        engine.transform(&animated).unwrap_err().code(),
        ErrorCode::ImageLimitExceeded
    );
    let mut non_jpeg_quality = job(Vec::new());
    non_jpeg_quality.output.quality = Some(80);
    assert_eq!(
        engine.transform(&non_jpeg_quality).unwrap_err().code(),
        ErrorCode::ImageOptionUnsupported
    );
    for quality in [0, 101] {
        let mut invalid_quality = job(Vec::new());
        invalid_quality.output = OutputOptions {
            format: RasterFormat::Jpeg,
            quality: Some(quality),
            anim: false,
        };
        assert_eq!(
            engine.transform(&invalid_quality).unwrap_err().code(),
            ErrorCode::ImageOptionUnsupported
        );
    }
}

#[test]
fn codec_parsers_geometry_and_compositing_cover_all_day1_edges() {
    assert_eq!(RasterFormat::Jpeg.mime_type(), "image/jpeg");
    assert_eq!(RasterFormat::Png.mime_type(), "image/png");
    assert_eq!(RasterFormat::Webp.mime_type(), "image/webp");
    assert_eq!(RasterFormat::Avif.mime_type(), "image/avif");
    assert_eq!(
        supported_input(image::ImageFormat::Jpeg).unwrap(),
        RasterFormat::Jpeg
    );
    assert_eq!(
        supported_input(image::ImageFormat::Png).unwrap(),
        RasterFormat::Png
    );
    assert_eq!(
        supported_input(image::ImageFormat::WebP).unwrap(),
        RasterFormat::Webp
    );
    assert_eq!(
        supported_input(image::ImageFormat::Avif)
            .unwrap_err()
            .code(),
        ErrorCode::ImageFormatUnsupported
    );
    assert_eq!(
        supported_input(image::ImageFormat::Gif).unwrap_err().code(),
        ErrorCode::ImageFormatUnsupported
    );

    assert!(!png_has_animation(b"not-png"));
    assert!(!png_has_animation(b"\x89PNG\r\n\x1a\n\0\0\0\x20IEND"));
    assert!(!webp_has_animation(b"not-webp"));
    assert!(!webp_has_animation(b"RIFF\0\0\0\0WEBPVP8X\xff\xff\xff\xff"));
    assert_eq!(scaled_dimension(10, 0, 7), 1);
    assert_eq!(scaled_dimension(u32::MAX, 1, u32::MAX), u32::MAX);

    for orientation in [
        Orientation::Rotate90,
        Orientation::Rotate270,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ] {
        assert_eq!(oriented_dimensions(3, 5, orientation), (5, 3));
    }
    assert_eq!(oriented_dimensions(3, 5, Orientation::NoTransforms), (3, 5));

    let expected = [
        (Gravity::Left, (0, 3)),
        (Gravity::Right, (6, 3)),
        (Gravity::Top, (3, 0)),
        (Gravity::Bottom, (3, 6)),
        (Gravity::TopLeft, (0, 0)),
        (Gravity::TopRight, (6, 0)),
        (Gravity::BottomLeft, (0, 6)),
        (Gravity::BottomRight, (6, 6)),
        (Gravity::Center, (3, 3)),
    ];
    for (gravity, offset) in expected {
        assert_eq!(crop_offset(10, 10, 4, 4, gravity), offset);
        assert_eq!(anchored_offset(10, 10, 4, 4, gravity), offset);
    }

    let mut transparent = Rgba([1, 2, 3, 0]);
    source_over(&mut transparent, Rgba([4, 5, 6, 0]), 1.0);
    assert_eq!(transparent, Rgba([0, 0, 0, 0]));
    let mut base = RgbaImage::from_pixel(1, 1, Rgba([9, 8, 7, 255]));
    composite_source_over(
        &mut base,
        &RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])),
        -1,
        -1,
        1.0,
    );
    assert_eq!(base.get_pixel(0, 0), &Rgba([9, 8, 7, 255]));

    assert_eq!(invalid_input().code(), ErrorCode::ImageInputInvalid);
    assert_eq!(option().code(), ErrorCode::ImageOptionUnsupported);
    assert_eq!(limit().code(), ErrorCode::ImageLimitExceeded);
    assert_eq!(unavailable().code(), ErrorCode::ImageUnavailable);
}
