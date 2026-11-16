//! Deterministic image operations. This does not send images to a provider or
//! infer their visual contents. Originals are preserved; every output is new.
use super::{
    artifacts::directory,
    binary::{self, Stage},
    parse_args, ToolContext, ToolOutcome,
};
use crate::{
    failure::ToolFailure,
    fsops::{self, PathLock},
};
use image::{metadata::Orientation, DynamicImage, ImageDecoder, ImageFormat, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{Cursor, Seek, SeekFrom, Write},
    path::Path,
    time::{Duration, Instant},
};

const MAX_INPUT: u64 = 16 * 1024 * 1024;
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_SIDE: u32 = 8192;
static IMAGE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
fn invalid(s: impl Into<String>) -> ToolFailure {
    ToolFailure::InvalidArguments(s.into())
}
fn unsupported(s: impl Into<String>) -> ToolFailure {
    ToolFailure::Unsupported(s.into())
}
fn io(e: impl std::fmt::Display) -> ToolFailure {
    ToolFailure::Io(e.to_string())
}
fn decode_error(e: image::ImageError) -> ToolFailure {
    invalid(format!("invalid or oversized image: {e}"))
}
fn dimensions(w: u32, h: u32) -> Result<(), ToolFailure> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS
    {
        return Err(invalid(
            "image dimensions must be 1–8192 per side and at most 16,777,216 pixels",
        ));
    }
    Ok(())
}
fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_SIDE);
    l.max_image_height = Some(MAX_SIDE);
    l.max_alloc = Some(128 * 1024 * 1024);
    l
}
struct Opened<'a> {
    decoder: Box<dyn ImageDecoder + 'a>,
    format: ImageFormat,
    animated: bool,
    orientation: Orientation,
    icc: bool,
}
fn open(bytes: &[u8]) -> Result<Opened<'_>, ToolFailure> {
    let format = image::guess_format(bytes).map_err(decode_error)?;
    let (mut decoder, animated): (Box<dyn ImageDecoder>, bool) = match format {
        ImageFormat::Png => {
            let d = image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits())
                .map_err(decode_error)?;
            let animated = d.is_apng().map_err(decode_error)?;
            (Box::new(d), animated)
        }
        ImageFormat::Jpeg => {
            let mut r = image::ImageReader::with_format(Cursor::new(bytes), format);
            r.limits(limits());
            (Box::new(r.into_decoder().map_err(decode_error)?), false)
        }
        ImageFormat::WebP => {
            let mut d =
                image::codecs::webp::WebPDecoder::new(Cursor::new(bytes)).map_err(decode_error)?;
            d.set_limits(limits()).map_err(decode_error)?;
            let animated = d.has_animation();
            (Box::new(d), animated)
        }
        _ => {
            return Err(unsupported(
                "image tools support PNG, JPEG and WebP inputs only",
            ))
        }
    };
    let (w, h) = decoder.dimensions();
    dimensions(w, h)?;
    if decoder.total_bytes() > 128 * 1024 * 1024 {
        return Err(invalid("decoded image exceeds 128 MiB"));
    }
    let orientation = decoder.orientation().map_err(decode_error)?;
    let icc = decoder.icc_profile().map_err(decode_error)?.is_some();
    Ok(Opened {
        decoder,
        format,
        animated,
        orientation,
        icc,
    })
}
fn metadata(opened: &Opened<'_>) -> Value {
    let (w, h) = opened.decoder.dimensions();
    let c = opened.decoder.color_type();
    json!({"format":format!("{:?}",opened.format).to_lowercase(),"width":w,"height":h,"pixels":u64::from(w)*u64::from(h),"color_type":format!("{c:?}"),"has_alpha":c.has_alpha(),"animated":opened.animated,"orientation":format!("{:?}",opened.orientation),"has_icc_profile":opened.icc,"transform_supported":!opened.animated&&!opened.icc&&matches!(c,image::ColorType::L8|image::ColorType::La8|image::ColorType::Rgb8|image::ColorType::Rgba8),"inspection":"metadata only; this is not visual understanding or full pixel validation"})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Info {
    path: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Format {
    Png,
    Jpeg,
    Webp,
}
impl Format {
    fn image(&self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
        }
    }
    fn matches_extension(&self, path: &str) -> bool {
        let extension = Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match self {
            Self::Png => extension == "png",
            Self::Jpeg => matches!(extension.as_str(), "jpg" | "jpeg"),
            Self::Webp => extension == "webp",
        }
    }
}
fn default_format() -> Format {
    Format::Png
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Resize {
        width: u32,
        height: u32,
    },
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Rotate {
        degrees: u16,
    },
    Convert {},
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transform {
    path: String,
    destination: String,
    expected_sha256: String,
    operation: Operation,
    #[serde(default = "default_format")]
    format: Format,
}

pub async fn info(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Info = parse_args(args)?;
    let _permit = IMAGE_SLOTS.try_acquire().map_err(|_| {
        invalid("image processing is busy; wait for a running image operation to finish")
    })?;
    let bytes = binary::read_source(ctx, "image_info", &args.path, MAX_INPUT)?;
    let opened = open(&bytes)?;
    let mut data = metadata(&opened);
    data["sha256"] = json!(fsops::sha256_bytes(&bytes));
    data["bytes"] = json!(bytes.len());
    Ok(ToolOutcome::read("Image metadata", "No pixels or metadata are sent to a website. Use transform_image for a new approved output. Animated, embedded-ICC and high-bit-depth transformation are unsupported.",Some(args.path)).with_data(data))
}

fn render(bytes: &[u8], op: &Operation, format: &Format) -> Result<(Vec<u8>, Value), ToolFailure> {
    let end = Instant::now() + Duration::from_secs(15);
    let opened = open(bytes)?;
    if opened.animated {
        return Err(unsupported(
            "animated PNG/WebP transforms are unsupported; no frames were discarded",
        ));
    }
    if opened.icc {
        return Err(unsupported("embedded ICC color profiles are unsupported for transforms; no color profile was discarded"));
    }
    if !matches!(
        opened.decoder.color_type(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::Rgb8
            | image::ColorType::Rgba8
    ) {
        return Err(unsupported(
            "only 8-bit image transforms are supported; high-bit-depth input was not converted",
        ));
    }
    let mut image = DynamicImage::from_decoder(opened.decoder).map_err(decode_error)?;
    image.apply_orientation(opened.orientation);
    let transformed = match op {
        Operation::Resize { width, height } => {
            dimensions(*width, *height)?;
            image.resize_exact(*width, *height, image::imageops::FilterType::Triangle)
        }
        Operation::Crop {
            x,
            y,
            width,
            height,
        } => {
            dimensions(*width, *height)?;
            if x.checked_add(*width).is_none_or(|v| v > image.width())
                || y.checked_add(*height).is_none_or(|v| v > image.height())
            {
                return Err(invalid("crop rectangle extends beyond the oriented image"));
            }
            image.crop_imm(*x, *y, *width, *height)
        }
        Operation::Rotate { degrees } => match degrees {
            90 => image.rotate90(),
            180 => image.rotate180(),
            270 => image.rotate270(),
            _ => return Err(invalid("rotation must be 90, 180 or 270 clockwise degrees")),
        },
        Operation::Convert {} => image,
    };
    if Instant::now() >= end {
        return Err(invalid("image processing exceeded its 15-second budget"));
    }
    // Normalize the supported 8-bit layouts explicitly, preserving alpha for
    // PNG/WebP. JPEG requires an opaque image; never silently flatten it.
    let output = if matches!(format, Format::Jpeg) {
        if transformed.color().has_alpha() && transformed.to_rgba8().pixels().any(|p| p.0[3] != 255)
        {
            return Err(unsupported(
                "JPEG cannot preserve transparency; choose PNG or WebP",
            ));
        }
        DynamicImage::ImageRgb8(transformed.to_rgb8())
    } else if transformed.color().has_alpha() {
        DynamicImage::ImageRgba8(transformed.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(transformed.to_rgb8())
    };
    let mut sink = ImageOutput {
        cursor: Cursor::new(vec![]),
        end,
    };
    output
        .write_to(&mut sink, format.image())
        .map_err(decode_error)?;
    let bytes = sink.cursor.into_inner();
    let verified = open(&bytes)?;
    if verified.decoder.dimensions() != (output.width(), output.height())
        || verified.format != format.image()
    {
        return Err(io("encoded image metadata failed verification"));
    }
    let data = json!({"width":output.width(),"height":output.height(),"format":format,"operation":op,"orientation_normalized":true,"metadata":"EXIF/XMP/text are not copied; ICC input is refused","lossy":matches!(format,Format::Jpeg),"resize":"resize uses exact dimensions; aspect ratio may change"});
    drop(verified);
    Ok((bytes, data))
}
struct ImageOutput {
    cursor: Cursor<Vec<u8>>,
    end: Instant,
}
impl Write for ImageOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if Instant::now() >= self.end
            || self.cursor.position().saturating_add(bytes.len() as u64) > MAX_OUTPUT
        {
            return Err(std::io::Error::other(
                "image output exceeds byte/time budget",
            ));
        }
        self.cursor.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Seek for ImageOutput {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(pos)
    }
}

pub async fn transform(ctx: &ToolContext<'_>, args: Value) -> Result<ToolOutcome, ToolFailure> {
    let args: Transform = parse_args(args)?;
    if !args.format.matches_extension(&args.destination) {
        return Err(invalid(
            "destination extension must match the requested png, jpeg or webp format",
        ));
    }
    let _permit = IMAGE_SLOTS.try_acquire().map_err(|_| {
        invalid("image processing is busy; wait for a running image operation to finish")
    })?;
    let initial = binary::target(ctx, "transform_image", &args.destination)?;
    let source = binary::read_source(ctx, "transform_image", &args.path, MAX_INPUT)?;
    if fsops::sha256_bytes(&source) != args.expected_sha256 {
        return Err(ToolFailure::Conflict(
            "source image changed; inspect it again".into(),
        ));
    }
    let (bytes, mut details) = render(&source, &args.operation, &args.format)?;
    details["source_sha256"] = json!(args.expected_sha256);
    details["destination"] = json!(initial.display());
    details["sha256"] = json!(fsops::sha256_bytes(&bytes));
    details["bytes"] = json!(bytes.len());
    ctx.app.request_approval(ctx.session_id,ctx.agent,"transform_image",&format!("Create a new transformed image from {}. Original preserved. Output metadata is stripped after orientation normalization; no binary undo.\n{}",args.path,serde_json::to_string_pretty(&details).map_err(io)?),Some(initial.display())).await?;
    let _lock = PathLock::acquire(&ctx.app.workspace.lock_dir, &initial.display()).map_err(io)?;
    let path = binary::target(ctx, "transform_image", &args.destination)?;
    if fsops::sha256_bytes(&binary::read_source(
        ctx,
        "transform_image",
        &args.path,
        MAX_INPUT,
    )?) != args.expected_sha256
    {
        return Err(ToolFailure::Conflict(
            "source image changed during approval".into(),
        ));
    }
    let mut stage = Stage::new(directory(path.absolute.parent().unwrap())?, false)?;
    details["staging_name"] = json!(stage.name.to_string_lossy());
    details["state"] = json!("prepared");
    let staged = (|| {
        ctx.app
            .audit_path_access(
                ctx.agent,
                ctx.session_id,
                "image.prepare",
                Some(path.display()),
                "allowed",
                &details.to_string(),
            )
            .map_err(io)?;
        stage.file.write_all(&bytes).map_err(io)?;
        binary::target(ctx, "transform_image", &args.destination)?;
        stage.publish(&path.absolute)
    })();
    binary::staged_result(ctx, "transform_image", &path.display(), &mut stage, staged)?;
    details["state"] = json!("completed");
    ctx.app
        .audit_path_access(
            ctx.agent,
            ctx.session_id,
            "tool.transform_image",
            Some(path.display()),
            "allowed",
            &details.to_string(),
        )
        .map_err(|e| {
            io(format!(
                "image published but audit finalization failed; inspect destination: {e}"
            ))
        })?;
    Ok(ToolOutcome::write("Image transformed into a new file","Original preserved. This does not attach the image to a web chat or establish visual understanding.",Some(path.display())).with_data(details))
}
