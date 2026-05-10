use image::imageops::FilterType;
use image::DynamicImage;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Result of a batch resize operation, including decode timing.
pub struct ResizeAllResult {
    /// Time spent decoding the source image.
    pub decode_elapsed: Duration,
    /// Per-spec results: (label, output_path, resize result).
    pub items: Vec<(String, PathBuf, Result<(), ImageError>)>,
}

/// Predefined output sizes for image transformations.
#[derive(Debug, Clone, Copy)]
pub struct ResizeSpec {
    pub width: u32,
    pub height: u32,
    pub label: &'static str,
}

/// Table thumbnail: 32x32
pub const TABLE_THUMB: ResizeSpec = ResizeSpec {
    width: 32,
    height: 32,
    label: "table_thumb",
};

/// Thumbnail: 80x80
pub const THUMBNAIL: ResizeSpec = ResizeSpec {
    width: 80,
    height: 80,
    label: "thumbnail",
};

/// Small preview: 250x250
pub const SMALL_PREVIEW: ResizeSpec = ResizeSpec {
    width: 250,
    height: 250,
    label: "small_preview",
};

/// Large preview: 800x600
pub const LARGE_PREVIEW: ResizeSpec = ResizeSpec {
    width: 800,
    height: 600,
    label: "large_preview",
};

/// High resolution: 6000x6000
pub const HIGH_RES: ResizeSpec = ResizeSpec {
    width: 6000,
    height: 6000,
    label: "high_res",
};

/// The three standard preview sizes generated for every upload.
pub const STANDARD_SIZES: [ResizeSpec; 4] = [TABLE_THUMB, THUMBNAIL, SMALL_PREVIEW, LARGE_PREVIEW];

/// Threshold in pixels: if either dimension of the original exceeds this,
/// a high-resolution derivative is also generated.
pub const HIGH_RES_THRESHOLD: u32 = 3000;

/// Resize an image file to fit within the given dimensions, preserving aspect ratio.
///
/// The output is written to `output_path` in the same format as the input.
/// Uses Lanczos3 filtering for high-quality downscaling.
/// Never upscales: if the image already fits within the spec, it is saved as-is.
pub fn resize_image(
    input_path: &Path,
    output_path: &Path,
    spec: &ResizeSpec,
) -> Result<(), ImageError> {
    let img = image::open(input_path).map_err(ImageError::Decode)?;
    resize_from_decoded(&img, output_path, spec)
}

/// Resize a pre-decoded image to fit within the given dimensions.
///
/// This avoids re-decoding the source when generating multiple derivatives.
/// Never upscales: if the image already fits within the spec, it is saved as-is.
pub fn resize_from_decoded(
    img: &DynamicImage,
    output_path: &Path,
    spec: &ResizeSpec,
) -> Result<(), ImageError> {
    if img.width() <= spec.width && img.height() <= spec.height {
        img.save(output_path).map_err(ImageError::Encode)?;
    } else {
        let resized = img.resize(spec.width, spec.height, FilterType::Lanczos3);
        resized.save(output_path).map_err(ImageError::Encode)?;
    }
    Ok(())
}

/// Returns the dimensions (width, height) of the image at `input_path`.
pub fn get_dimensions(input_path: &Path) -> Result<(u32, u32), ImageError> {
    image::image_dimensions(input_path).map_err(ImageError::Decode)
}

/// Returns `true` if the image at `input_path` exceeds [`HIGH_RES_THRESHOLD`]
/// in either dimension.
pub fn needs_high_res(input_path: &Path) -> Result<bool, ImageError> {
    let (w, h) = image::image_dimensions(input_path).map_err(ImageError::Decode)?;
    Ok(w > HIGH_RES_THRESHOLD || h > HIGH_RES_THRESHOLD)
}

/// Generate all standard derivatives for an uploaded image.
///
/// Decodes the source image once and resizes all derivatives in parallel
/// using rayon. Returns a list of `(label, output_path)` pairs for each
/// derivative that was successfully created. If the original exceeds
/// [`HIGH_RES_THRESHOLD`], a high-resolution derivative is included as well.
///
/// `output_dir` is the directory where derived files will be written.
/// Output filenames are `{stem}_{width}x{height}.{ext}`.
pub fn generate_derivatives(
    input_path: &Path,
    output_dir: &Path,
) -> Result<Vec<(String, PathBuf)>, ImageError> {
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or(ImageError::InvalidPath)?;
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("jpg");

    // Decode the source image once.
    let img = image::open(input_path).map_err(ImageError::Decode)?;

    let mut specs: Vec<ResizeSpec> = STANDARD_SIZES.to_vec();
    if img.width() > HIGH_RES_THRESHOLD || img.height() > HIGH_RES_THRESHOLD {
        specs.push(HIGH_RES);
    }

    // Resize all specs in parallel — each closure borrows &img (read-only).
    let results: Vec<Result<(String, PathBuf), ImageError>> = specs
        .par_iter()
        .map(|spec| {
            let out_name = format!("{stem}_{}x{}.{ext}", spec.width, spec.height);
            let out_path = output_dir.join(&out_name);
            resize_from_decoded(&img, &out_path, spec)?;
            Ok((spec.label.to_string(), out_path))
        })
        .collect();

    // Collect successes, propagate first error.
    results.into_iter().collect()
}

/// Decode the image at `input_path` and resize it to all given `specs` in
/// parallel using rayon.
///
/// Returns a list of `(label, output_path)` for each successfully resized
/// derivative. Errors for individual specs are returned per-entry so the
/// caller can handle partial failures.
pub fn resize_all_parallel(
    input_path: &Path,
    specs: &[(ResizeSpec, PathBuf)],
) -> Result<ResizeAllResult, ImageError> {
    let t0 = std::time::Instant::now();
    let img = image::open(input_path).map_err(ImageError::Decode)?;
    let decode_elapsed = t0.elapsed();

    let items: Vec<(String, PathBuf, Result<(), ImageError>)> = specs
        .par_iter()
        .map(|(spec, out_path)| {
            let result = resize_from_decoded(&img, out_path, spec);
            (spec.label.to_string(), out_path.clone(), result)
        })
        .collect();

    Ok(ResizeAllResult { decode_elapsed, items })
}

/// Decode the image at `input_path` and resize it to all given `specs`
/// sequentially (single-threaded). Same interface as [`resize_all_parallel`].
pub fn resize_all_sequential(
    input_path: &Path,
    specs: &[(ResizeSpec, PathBuf)],
) -> Result<ResizeAllResult, ImageError> {
    let t0 = std::time::Instant::now();
    let img = image::open(input_path).map_err(ImageError::Decode)?;
    let decode_elapsed = t0.elapsed();

    let items: Vec<(String, PathBuf, Result<(), ImageError>)> = specs
        .iter()
        .map(|(spec, out_path)| {
            let result = resize_from_decoded(&img, out_path, spec);
            (spec.label.to_string(), out_path.clone(), result)
        })
        .collect();

    Ok(ResizeAllResult { decode_elapsed, items })
}

/// Errors that can occur during image processing.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("failed to decode image: {0}")]
    Decode(image::ImageError),
    #[error("failed to encode image: {0}")]
    Encode(image::ImageError),
    #[error("invalid file path")]
    InvalidPath,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_image_path() -> PathBuf {
        // Use a small test image from the assets or create one in-memory
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        // Create a 100x100 red image
        let img = image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        // Leak the tempdir so the file persists for the test
        std::mem::forget(dir);
        path
    }

    fn large_test_image_path() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        let img = image::RgbImage::from_fn(4000, 3500, |_, _| image::Rgb([0, 128, 255]));
        img.save(&path).unwrap();
        std::mem::forget(dir);
        path
    }

    #[test]
    fn resize_creates_output() {
        let input = test_image_path();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("thumb.png");

        resize_image(&input, &output, &THUMBNAIL).unwrap();
        assert!(output.exists());

        let dims = image::image_dimensions(&output).unwrap();
        // 100x100 resized to fit 80x80 → 80x80
        assert!(dims.0 <= 80 && dims.1 <= 80);
    }

    #[test]
    fn resize_does_not_upscale() {
        let input = test_image_path(); // 100x100
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("large.png");

        let big_spec = ResizeSpec { width: 6000, height: 6000, label: "big" };
        resize_image(&input, &output, &big_spec).unwrap();
        assert!(output.exists());

        let dims = image::image_dimensions(&output).unwrap();
        // Must not exceed original 100x100
        assert_eq!(dims, (100, 100));
    }

    #[test]
    fn needs_high_res_small_image() {
        let input = test_image_path();
        assert!(!needs_high_res(&input).unwrap());
    }

    #[test]
    fn needs_high_res_large_image() {
        let input = large_test_image_path();
        assert!(needs_high_res(&input).unwrap());
    }

    #[test]
    fn generate_derivatives_standard() {
        let input = test_image_path();
        let dir = tempfile::tempdir().unwrap();

        let results = generate_derivatives(&input, dir.path()).unwrap();
        // Small image: 3 standard sizes, no high-res
        assert_eq!(results.len(), 4);
        for (_, path) in &results {
            assert!(path.exists());
        }
    }

    #[test]
    fn generate_derivatives_with_high_res() {
        let input = large_test_image_path();
        let dir = tempfile::tempdir().unwrap();

        let results = generate_derivatives(&input, dir.path()).unwrap();
        // Large image: 4 standard + 1 high-res
        assert_eq!(results.len(), 5);
        let labels: Vec<&str> = results.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"high_res"));
    }
}
