use super::*;
use image::ImageEncoder;
use objc2::{rc::autoreleasepool, AnyThread};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGPDFBox, CGPDFPage,
};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize};
use objc2_pdf_kit::{PDFDisplayBox, PDFDocument};

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
            "PDF worker filesystem/network restrictions unavailable: {detail}"
        )));
    }
    Ok(())
}

pub(super) fn process(bytes: &[u8], request: Request) -> Result<Value, ToolFailure> {
    restrict_worker()?;
    // Only data-backed, noninteractive PDFDocument/PDFPage APIs are used.
    // No document URL, PDFView, action, relationship or script API is called.
    autoreleasepool(|_| unsafe {
        let data = NSData::with_bytes(bytes);
        let document = PDFDocument::initWithData(PDFDocument::alloc(), &data)
            .ok_or_else(|| unsupported("PDFKit could not read this PDF"))?;
        if document.isEncrypted() || document.isLocked() {
            return Err(unsupported(
                "encrypted PDFs are refused; no password or decryption workflow is available",
            ));
        }
        if !document.allowsCopying() {
            return Err(unsupported("PDF disallows content copying"));
        }
        let count = document.pageCount();
        if !(1..=200).contains(&count) {
            return Err(unsupported("PDF must contain 1–200 pages"));
        }
        let mut info = Vec::new();
        let mut text = Text {
            text: String::new(),
            pages: Vec::new(),
        };
        let mut images = Vec::new();
        let mut png_total = 0usize;
        if let Request::Render {
            first_page,
            last_page,
            max_dimension,
        } = &request
        {
            if *first_page == 0
                || last_page < first_page
                || last_page - first_page >= 4
                || *last_page > count
                || !(128..=1600).contains(max_dimension)
            {
                return Err(invalid("PDF worker page or dimension limit exceeded"));
            }
        }
        for index in 0..count {
            let page = document
                .pageAtIndex(index)
                .ok_or_else(|| unsupported("PDF page could not be loaded"))?;
            let bounds = page.boundsForBox(PDFDisplayBox::CropBox);
            let rotation = page.rotation().rem_euclid(360);
            if !bounds.origin.x.is_finite()
                || !bounds.origin.y.is_finite()
                || !bounds.size.width.is_finite()
                || !bounds.size.height.is_finite()
                || bounds.size.width <= 0.0
                || bounds.size.height <= 0.0
                || bounds.size.width > 20000.0
                || bounds.size.height > 20000.0
                || rotation % 90 != 0
            {
                return Err(unsupported("PDF page has unsupported bounds or rotation"));
            }
            match request {
                Request::Info => info.push(json!({"page":index+1,"width_points":bounds.size.width,"height_points":bounds.size.height,"rotation_degrees":rotation})),
                Request::Text => {
                    let start=text.text.len();
                    if let Some(s)=page.string() {
                        if s.length()>256000 { return Err(unsupported("PDF text exceeds 256,000-byte budget")); }
                        let s=s.to_string();
                        if text.text.len()+s.len()+1>256000 { return Err(unsupported("PDF text exceeds 256,000-byte budget")); }
                        text.text.push_str(&s);
                        if !s.is_empty() && !s.ends_with('\n') { text.text.push('\n'); }
                    }
                    text.pages.push(PageText{page:index+1,start,end:text.text.len()});
                },
                Request::Render{first_page,last_page,max_dimension} => {
                    if index+1<first_page || index+1>last_page {continue;}
                    let cg=page.pageRef().ok_or_else(||unsupported("PDF page has no CoreGraphics content"))?;
                    let rect=CGPDFPage::box_rect(Some(&cg),CGPDFBox::CropBox);
                    let (w,h)=if rotation==90||rotation==270 {(rect.size.height,rect.size.width)} else {(rect.size.width,rect.size.height)};
                    if !w.is_finite() || !h.is_finite() || w<=0.0 || h<=0.0 || w>20000.0 || h>20000.0 {return Err(unsupported("invalid CoreGraphics page bounds"));}
                    let scale=f64::from(max_dimension)/w.max(h);
                    let width=(w*scale).round().max(1.0) as u32;
                    let height=(h*scale).round().max(1.0) as u32;
                    let mut rgba=vec![255u8; width as usize*height as usize*4];
                    let color=CGColorSpace::new_device_rgb().ok_or_else(||io("RGB color space unavailable"))?;
                    // Pixel buffer remains allocated and exclusively borrowed until context is dropped.
                    let context=CGBitmapContextCreate(rgba.as_mut_ptr().cast(),width as usize,height as usize,8,width as usize*4,Some(&color),CGImageAlphaInfo::PremultipliedLast.0|CGImageByteOrderInfo::Order32Big.0).ok_or_else(||io("PDF bitmap context unavailable"))?;
                    let target=NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(f64::from(width),f64::from(height)));
                    CGContext::set_rgb_fill_color(Some(&context),1.0,1.0,1.0,1.0);
                    CGContext::fill_rect(Some(&context),target);
                    // DrawingTransform centers smaller pages without upscaling.
                    // Apply pixel scale explicitly, then crop/rotate in page units.
                    CGContext::scale_ctm(Some(&context),scale,scale);
                    let page_target=NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(f64::from(width)/scale,f64::from(height)/scale));
                    CGContext::concat_ctm(Some(&context),CGPDFPage::drawing_transform(Some(&cg),CGPDFBox::CropBox,page_target,0,true));
                    CGContext::draw_pdf_page(Some(&context),Some(&cg));
                    drop(context);
                    let mut png=Vec::new();
                    image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba,width,height,image::ExtendedColorType::Rgba8).map_err(io)?;
                    png_total+=png.len();
                    if png_total>16*1024*1024 { return Err(unsupported("PDF PNG output exceeds 16 MiB")); }
                    images.push(PageImage{page:index+1,width,height,png:STANDARD.encode(png)});
                },
            }
        }
        Ok(match request {
            Request::Info => json!({"page_count":count,"pages":info}),
            Request::Text => json!({"page_count":count,"document":text}),
            Request::Render { .. } => json!({"page_count":count,"images":images}),
        })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_restrictions_deny_user_file_reads_writes_and_network() {
        if let Ok(root) = std::env::var("UAB_PDF_SANDBOX_PROBE") {
            super::restrict_worker().unwrap();
            assert!(std::fs::read(std::path::Path::new(&root).join("canary")).is_err());
            assert!(
                std::fs::write(std::path::Path::new(&root).join("output"), "forbidden").is_err()
            );
            let address = std::env::var("UAB_PDF_SANDBOX_PORT")
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
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","tools::pdf::macos::tests::worker_restrictions_deny_user_file_reads_writes_and_network","--nocapture"])
            .env("UAB_PDF_SANDBOX_PROBE",temp.path())
            .env("UAB_PDF_SANDBOX_PORT",listener.local_addr().unwrap().to_string()).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!temp.path().join("output").exists());
    }
}
