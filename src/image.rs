use std::path::{Path, PathBuf};
use std::time::Duration;

/// Result of a batch resize operation.
pub struct ResizeAllResult {
    /// Time spent on the first thumbnail call (proxy for decode time).
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

/// The standard preview sizes generated for every upload.
pub const STANDARD_SIZES: [ResizeSpec; 4] = [TABLE_THUMB, THUMBNAIL, SMALL_PREVIEW, LARGE_PREVIEW];

/// Threshold in pixels: if either dimension of the original exceeds this,
/// a high-resolution derivative is also generated.
pub const HIGH_RES_THRESHOLD: u32 = 3000;

// ── libvips backend (Linux / macOS) ─────────────────────────────────────────

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{ImageError, ResizeSpec};
    use std::path::Path;

    pub fn get_dimensions(input_path: &Path) -> Result<(u32, u32), ImageError> {
        let input_str = input_path.to_str().ok_or(ImageError::InvalidPath)?;
        let img = libvips::VipsImage::new_from_file(input_str)
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        Ok((img.get_width() as u32, img.get_height() as u32))
    }

    /// Uses libvips `thumbnail` for fused decode+resize (fastest path).
    /// Never upscales: if the image already fits, it passes through unchanged.
    pub fn resize_to_png(
        input_path: &Path,
        output_path: &Path,
        spec: &ResizeSpec,
    ) -> Result<(), ImageError> {
        let input_str = input_path.to_str().ok_or(ImageError::InvalidPath)?;
        let output_str = output_path.to_str().ok_or(ImageError::InvalidPath)?;

        let img = libvips::VipsImage::new_from_file(input_str)
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        let (w, h) = (img.get_width() as u32, img.get_height() as u32);

        if w <= spec.width && h <= spec.height {
            libvips::ops::pngsave(&img, output_str)
                .map_err(|e| ImageError::Encode(e.to_string()))?;
        } else {
            // Compute the constraining width so both width AND height fit.
            // thumbnail() only takes a width param; we derive the effective
            // width that respects both the width and height constraints.
            let scale_w = spec.width as f64 / w as f64;
            let scale_h = spec.height as f64 / h as f64;
            let scale = scale_w.min(scale_h);
            let effective_width = (w as f64 * scale).round() as i32;

            let resized = libvips::ops::thumbnail(input_str, effective_width)
                .map_err(|e| ImageError::Decode(e.to_string()))?;
            libvips::ops::pngsave(&resized, output_str)
                .map_err(|e| ImageError::Encode(e.to_string()))?;
        }

        Ok(())
    }
}

// ── fast_image_resize + image crate backend (Windows) ───────────────────────
//
// `image` handles decode/encode; `fast_image_resize` does the actual resize
// step using SIMD (AVX2/SSE4.1 on x86-64), giving 5-10× speedup over
// `image`'s scalar Lanczos3 for large inputs.

#[cfg(target_os = "windows")]
mod platform {
    use super::{ImageError, ResizeSpec};
    use fast_image_resize::{
        FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
        images::Image,
    };
    use std::path::Path;

    fn open_image(input_path: &Path) -> Result<image::DynamicImage, ImageError> {
        // `image::open` uses a default 512 MiB allocation cap that triggers
        // "Memory limit exceeded" for large images. Disable it: we trust our
        // own upload size cap (10 GB) as the effective ceiling.
        let mut reader = image::io::Reader::open(input_path)
            .map_err(|e| ImageError::Decode(e.to_string()))?
            .with_guessed_format()
            .map_err(|e| ImageError::Decode(e.to_string()))?;
        reader.limits(image::io::Limits::no_limits());
        reader.decode().map_err(|e| ImageError::Decode(e.to_string()))
    }

    pub fn get_dimensions(input_path: &Path) -> Result<(u32, u32), ImageError> {
        image::image_dimensions(input_path).map_err(|e| ImageError::Decode(e.to_string()))
    }

    /// Decode with the `image` crate, resize with `fast_image_resize` (SIMD
    /// Lanczos3), encode as PNG. Never upscales.
    pub fn resize_to_png(
        input_path: &Path,
        output_path: &Path,
        spec: &ResizeSpec,
    ) -> Result<(), ImageError> {
        let img = open_image(input_path)?;
        let (orig_w, orig_h) = (img.width(), img.height());

        if orig_w <= spec.width && orig_h <= spec.height {
            img.save(output_path).map_err(|e| ImageError::Encode(e.to_string()))?;
            return Ok(());
        }

        // Compute target dimensions that fit within spec, preserving aspect ratio.
        let scale = (spec.width as f64 / orig_w as f64)
            .min(spec.height as f64 / orig_h as f64);
        let target_w = ((orig_w as f64 * scale).round() as u32).max(1);
        let target_h = ((orig_h as f64 * scale).round() as u32).max(1);

        let rgba = img.into_rgba8();
        let src = Image::from_vec_u8(orig_w, orig_h, rgba.into_raw(), PixelType::U8x4)
            .map_err(|e| ImageError::Decode(e.to_string()))?;

        let mut dst = Image::new(target_w, target_h, PixelType::U8x4);

        Resizer::new()
            .resize(
                &src,
                &mut dst,
                &ResizeOptions::new()
                    .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
            )
            .map_err(|e| ImageError::Encode(e.to_string()))?;

        let out = image::RgbaImage::from_raw(target_w, target_h, dst.into_vec())
            .ok_or_else(|| ImageError::Encode("buffer size mismatch after resize".to_string()))?;

        out.save(output_path).map_err(|e| ImageError::Encode(e.to_string()))?;

        Ok(())
    }
}

// ── public API (platform-independent) ───────────────────────────────────────

/// Returns the dimensions (width, height) of the image at `input_path`.
pub fn get_dimensions(input_path: &Path) -> Result<(u32, u32), ImageError> {
    platform::get_dimensions(input_path)
}

/// Resize an image to fit within the given dimensions and save as PNG.
///
/// Never upscales: if the image already fits, it passes through unchanged.
pub fn resize_to_png(
    input_path: &Path,
    output_path: &Path,
    spec: &ResizeSpec,
) -> Result<(), ImageError> {
    platform::resize_to_png(input_path, output_path, spec)
}

/// Returns `true` if the image at `input_path` exceeds [`HIGH_RES_THRESHOLD`]
/// in either dimension.
pub fn needs_high_res(input_path: &Path) -> Result<bool, ImageError> {
    let (w, h) = get_dimensions(input_path)?;
    Ok(w > HIGH_RES_THRESHOLD || h > HIGH_RES_THRESHOLD)
}

/// Generate all standard derivatives for an uploaded image.
///
/// Returns a list of `(label, output_path)` pairs for each derivative created.
/// If the original exceeds [`HIGH_RES_THRESHOLD`], a high-resolution derivative
/// is included. Output filenames are `{stem}_{width}x{height}.png`.
pub fn generate_derivatives(
    input_path: &Path,
    output_dir: &Path,
) -> Result<Vec<(String, PathBuf)>, ImageError> {
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or(ImageError::InvalidPath)?;

    let (w, h) = get_dimensions(input_path)?;

    let mut specs: Vec<ResizeSpec> = STANDARD_SIZES.to_vec();
    if w > HIGH_RES_THRESHOLD || h > HIGH_RES_THRESHOLD {
        specs.push(HIGH_RES);
    }

    let mut results = Vec::new();
    for spec in &specs {
        let out_name = format!("{stem}_{}x{}.png", spec.width, spec.height);
        let out_path = output_dir.join(&out_name);
        resize_to_png(input_path, &out_path, spec)?;
        results.push((spec.label.to_string(), out_path));
    }

    Ok(results)
}

/// Resize the image to all given `specs` sequentially.
///
/// Returns a `ResizeAllResult` with per-spec outcomes. `decode_elapsed`
/// measures the first resize call (which includes the initial decode).
pub fn resize_all(
    input_path: &Path,
    specs: &[(ResizeSpec, PathBuf)],
) -> Result<ResizeAllResult, ImageError> {
    let mut items: Vec<(String, PathBuf, Result<(), ImageError>)> = Vec::with_capacity(specs.len());
    let mut decode_elapsed = Duration::ZERO;

    for (i, (spec, out_path)) in specs.iter().enumerate() {
        let t0 = std::time::Instant::now();
        let result = resize_to_png(input_path, out_path, spec);
        if i == 0 {
            decode_elapsed = t0.elapsed();
        }
        items.push((spec.label.to_string(), out_path.clone(), result));
    }

    Ok(ResizeAllResult {
        decode_elapsed,
        items,
    })
}

/// Errors that can occur during image processing.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("failed to decode image: {0}")]
    Decode(String),
    #[error("failed to encode image: {0}")]
    Encode(String),
    #[error("invalid file path")]
    InvalidPath,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // libvips must be initialised once per process on non-Windows.
    #[cfg(not(target_os = "windows"))]
    static VIPS_INIT: std::sync::Once = std::sync::Once::new();

    fn init_vips() {
        #[cfg(not(target_os = "windows"))]
        VIPS_INIT.call_once(|| {
            let _app = libvips::VipsApp::new("rapid-test", false)
                .expect("failed to init libvips for tests");
            std::mem::forget(_app);
        });
    }

    fn test_image_path() -> PathBuf {
        init_vips();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        let img = image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([255, 0, 0]));
        img.save(&path).unwrap();
        std::mem::forget(dir);
        path
    }

    fn large_test_image_path() -> PathBuf {
        init_vips();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        let img = image::RgbImage::from_fn(4000, 3500, |_, _| image::Rgb([0, 128, 255]));
        img.save(&path).unwrap();
        std::mem::forget(dir);
        path
    }

    #[test]
    fn resize_creates_png_output() {
        let input = test_image_path();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("thumb.png");

        resize_to_png(&input, &output, &THUMBNAIL).unwrap();
        assert!(output.exists());

        let dims = image::image_dimensions(&output).unwrap();
        assert!(dims.0 <= 80 && dims.1 <= 80);
    }

    #[test]
    fn resize_does_not_upscale() {
        let input = test_image_path(); // 100x100
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("large.png");

        let big_spec = ResizeSpec { width: 6000, height: 6000, label: "big" };
        resize_to_png(&input, &output, &big_spec).unwrap();
        assert!(output.exists());

        let dims = image::image_dimensions(&output).unwrap();
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
        assert_eq!(results.len(), 4);
        for (_, path) in &results {
            assert!(path.exists());
            assert_eq!(path.extension().unwrap(), "png");
        }
    }

    #[test]
    fn generate_derivatives_with_high_res() {
        let input = large_test_image_path();
        let dir = tempfile::tempdir().unwrap();

        let results = generate_derivatives(&input, dir.path()).unwrap();
        assert_eq!(results.len(), 5);
        let labels: Vec<&str> = results.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"high_res"));
    }

    #[test]
    fn get_dimensions_returns_correct_size() {
        let input = test_image_path();
        let (w, h) = get_dimensions(&input).unwrap();
        assert_eq!((w, h), (100, 100));
    }

    #[test]
    fn resize_all_returns_results_for_each_spec() {
        let input = test_image_path();
        let dir = tempfile::tempdir().unwrap();
        let specs: Vec<(ResizeSpec, PathBuf)> = vec![
            (THUMBNAIL, dir.path().join("thumb.png")),
            (SMALL_PREVIEW, dir.path().join("small.png")),
        ];

        let result = resize_all(&input, &specs).unwrap();
        assert_eq!(result.items.len(), 2);
        assert!(result.decode_elapsed > Duration::ZERO);
        for (_, path, res) in &result.items {
            assert!(res.is_ok());
            assert!(path.exists());
        }
    }

    #[test]
    fn resize_respects_height_constraint_for_portrait() {
        init_vips();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("portrait.png");
        // 200x800 portrait image
        let img = image::RgbImage::from_fn(200, 800, |_, _| image::Rgb([0, 255, 0]));
        img.save(&input).unwrap();

        let output = dir.path().join("out.png");
        // Spec is 800x600 — height (600) should be the constraint
        resize_to_png(&input, &output, &LARGE_PREVIEW).unwrap();

        let (w, h) = image::image_dimensions(&output).unwrap();
        assert!(h <= 600, "height {h} should be <= 600");
        assert!(w <= 800, "width {w} should be <= 800");
    }

    #[test]
    fn resize_respects_width_constraint_for_landscape() {
        init_vips();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("landscape.png");
        // 2000x500 landscape image
        let img = image::RgbImage::from_fn(2000, 500, |_, _| image::Rgb([255, 255, 0]));
        img.save(&input).unwrap();

        let output = dir.path().join("out.png");
        // Spec is 800x600 — width (800) should be the constraint
        resize_to_png(&input, &output, &LARGE_PREVIEW).unwrap();

        let (w, h) = image::image_dimensions(&output).unwrap();
        assert!(w <= 800, "width {w} should be <= 800");
        assert!(h <= 600, "height {h} should be <= 600");
    }
}
