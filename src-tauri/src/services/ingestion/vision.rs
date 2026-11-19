//! Apple Vision as a second reader for table cells (macOS). The PDF's text layer is the first
//! reader. Vision reads the pixels of a cell, so the two disagree where a text layer is wrong
//! (a minus stored as `2`, a µ stored as `m`). Vision's text never replaces the layer's: a
//! disagreement only marks the cell unverified. Language correction is off, because a corrector
//! that "fixes" a number is the kind of guess this app does not make.

use image::RgbImage;

/// Vision exists only on macOS. Elsewhere nothing is checked, which is not the same as agreeing.
pub const AVAILABLE: bool = cfg!(target_os = "macos");

#[cfg(target_os = "macos")]
mod apple {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequestTextRecognitionLevel};

    pub fn read_lines(png: &[u8]) -> Result<Vec<String>, String> {
        autoreleasepool(|_| {
            let data = NSData::with_bytes(png);
            let options = NSDictionary::<_, AnyObject>::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            request.setUsesLanguageCorrection(false);
            let requests = NSArray::from_retained_slice(&[objc2::rc::Retained::into_super(objc2::rc::Retained::into_super(request.clone()))]);
            handler.performRequests_error(&requests).map_err(|e| e.localizedDescription().to_string())?;
            let Some(found) = request.results() else { return Ok(vec![]) };
            Ok(found
                .iter()
                .filter_map(|o| o.topCandidates(1).firstObject().map(|t| t.string().to_string()))
                .collect())
        })
    }
}

/// The lines Vision reads in an image, in the order it reports them. Empty on other platforms.
pub fn read_lines(img: &RgbImage) -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
        apple::read_lines(&png)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = img;
        Ok(vec![])
    }
}
