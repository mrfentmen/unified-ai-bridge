use super::*;
use image::{ImageDecoder, ImageEncoder};
use objc2::{rc::autoreleasepool, AnyThread};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};
use std::io::Cursor;

fn restrict_worker() -> Result<(), ToolFailure> {
    use std::ffi::{CStr, CString};
    // Seatbelt is a deprecated but still available macOS C API. Fail closed if
    // unavailable/refused. Only this short-lived worker receives the policy.
    #[link(name = "sandbox")]
    unsafe extern "C" {
        fn sandbox_init(
            profile: *const libc::c_char,
            flags: u64,
            error: *mut *mut libc::c_char,
        ) -> libc::c_int;
        fn sandbox_free_error(error: *mut libc::c_char);
    }
    let profile = CString::new(
        r#"(version 1)
        (allow default)
        (deny network*) (deny file-write*) (deny process-exec) (deny process-fork)
        (deny file-read-data (require-all
          (require-not (subpath "/System/Library")) (require-not (subpath "/usr/lib"))
          (require-not (subpath "/Library/Fonts")) (require-not (subpath "/usr/share"))))"#,
    )
    .map_err(io)?;
    let mut error = std::ptr::null_mut();
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    let detail = if error.is_null() {
        String::new()
    } else {
        let s = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        unsafe { sandbox_free_error(error) };
        s
    };
    if result != 0 {
        return Err(unsupported(format!(
            "OCR worker filesystem/network restrictions unavailable: {detail}"
        )));
    }
    Ok(())
}

#[allow(deprecated)] // CPU-only is retained for Intel compatibility.
pub(super) fn process(bytes: &[u8], limit: usize) -> Result<Value, ToolFailure> {
    restrict_worker()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits)
        .map_err(|e| unsupported(format!("invalid or oversized OCR PNG: {e}")))?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > 4_000_000
        || decoder.is_apng().map_err(io)?
    {
        return Err(unsupported(
            "OCR needs a static PNG with at most 4,000,000 pixels, 4096 per side",
        ));
    }
    if !matches!(
        decoder.color_type(),
        image::ColorType::L8
            | image::ColorType::La8
            | image::ColorType::Rgb8
            | image::ColorType::Rgba8
    ) || decoder.icc_profile().map_err(io)?.is_some()
        || decoder.orientation().map_err(io)? != image::metadata::Orientation::NoTransforms
    {
        return Err(unsupported("OCR refuses high-bit-depth, embedded-ICC or oriented PNG; create a supported plain PNG first"));
    }
    let mut pixels = image::DynamicImage::from_decoder(decoder)
        .map_err(io)?
        .into_rgba8();
    // White preview background; completely transparent text remains invisible.
    for pixel in pixels.pixels_mut() {
        let a = u16::from(pixel.0[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        pixel.0[3] = 255;
    }
    let mut png = vec![];
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(io)?;
    autoreleasepool(|_| unsafe {
        if objc2::runtime::AnyClass::get(c"VNRecognizeTextRequest").is_none() {
            return Err(unsupported(
                "Vision text recognition requires macOS 10.15 or later",
            ));
        }
        let request = VNRecognizeTextRequest::new();
        request.setRevision(1);
        request.setUsesCPUOnly(true);
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Fast);
        request.setUsesLanguageCorrection(false);
        request.setRecognitionLanguages(&NSArray::from_slice(&[&*NSString::from_str("en-US")]));
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &NSData::with_bytes(&png),
            &NSDictionary::new(),
        );
        handler
            .performRequests_error(&NSArray::<VNRequest>::from_slice(&[&*request]))
            .map_err(|e| unsupported(format!("Vision fast English backend unavailable: {e}")))?;
        let observations = request
            .results()
            .ok_or_else(|| unsupported("Vision omitted OCR results"))?;
        let mut lines = vec![];
        let mut text = String::new();
        let mut truncated = false;
        for i in 0..observations.len() {
            if lines.len() >= 200 {
                truncated = true;
                break;
            }
            let observed = observations.objectAtIndex(i);
            let candidates = observed.topCandidates(1);
            if candidates.len() == 0 {
                continue;
            }
            let candidate = candidates.objectAtIndex(0);
            let native = candidate.string();
            if native.length() > 24000 {
                truncated = true;
                break;
            }
            let s = native
                .to_string()
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect::<String>();
            if text.len() + s.len() + 1 > limit {
                truncated = true;
                break;
            }
            let rect = observed.boundingBox();
            let confidence = candidate.confidence();
            if !confidence.is_finite()
                || [
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                ]
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err(unsupported(
                    "Vision returned invalid OCR geometry or confidence",
                ));
            }
            let entry = json!({"text":s,"confidence":confidence,"box":{"x":rect.origin.x,"y":rect.origin.y,"width":rect.size.width,"height":rect.size.height}});
            lines.push(entry);
            if serde_json::to_vec(&lines).map_err(io)?.len() > 24000 {
                lines.pop();
                truncated = true;
                break;
            }
            text.push_str(&s);
            text.push('\n');
        }
        Ok(
            json!({"backend":"macOS Vision","mode":"fast_cpu","revision":1,"language":"en-US","language_correction":false,"width":width,"height":height,"text":text,"lines":lines,"observation_count":observations.len(),"truncated":truncated,"box_coordinates":"normalized 0–1, origin at bottom left of white-composited input","notice":"OCR may misread text. Confidence is a backend score, not proof of accuracy. Reading order may differ from the page. No recognized text does not prove the image contains none. This is local text recognition, not image understanding or attachment delivery.","continue_hint":if truncated {Some("Crop a smaller region into a new image with transform_image, then run OCR on that file.")}else{None}}),
        )
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_restrictions_deny_user_file_reads_writes_and_network() {
        if let Ok(root) = std::env::var("UAB_OCR_SANDBOX_PROBE") {
            super::restrict_worker().unwrap();
            assert!(std::fs::read(std::path::Path::new(&root).join("canary")).is_err());
            assert!(
                std::fs::write(std::path::Path::new(&root).join("output"), "forbidden").is_err()
            );
            let address = std::env::var("UAB_OCR_SANDBOX_PORT")
                .unwrap()
                .parse()
                .unwrap();
            assert!(std::net::TcpStream::connect_timeout(
                &address,
                std::time::Duration::from_secs(1)
            )
            .is_err());
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("canary"), "private test content").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","tools::ocr::macos::tests::worker_restrictions_deny_user_file_reads_writes_and_network","--nocapture"]).env("UAB_OCR_SANDBOX_PROBE",temp.path()).env("UAB_OCR_SANDBOX_PORT",listener.local_addr().unwrap().to_string()).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!temp.path().join("output").exists());
    }
}
