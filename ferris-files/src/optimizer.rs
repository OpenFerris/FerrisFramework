//! Image optimization

use ferris_core::{Error, FileError, Result, ValidationError};
use image::{DynamicImage, GenericImageView, ImageOutputFormat};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// Optimization options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationOptions {
    pub quality: u8,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub format: Option<OptimizationFormat>,
    pub strip_metadata: bool,
    pub progressive: bool,
}

impl Default for OptimizationOptions {
    fn default() -> Self {
        Self {
            quality: 85,
            max_width: None,
            max_height: None,
            format: None,
            strip_metadata: true,
            progressive: true,
        }
    }
}

/// Output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptimizationFormat {
    Jpeg,
    Png,
    WebP,
    Avif,
    Keep,
}

/// Image optimizer
pub struct ImageOptimizer {
    options: OptimizationOptions,
}

impl ImageOptimizer {
    pub fn new(options: OptimizationOptions) -> Self {
        Self { options }
    }

    pub fn with_quality(mut self, quality: u8) -> Self {
        self.options.quality = quality;
        self
    }

    pub fn with_max_dimensions(mut self, max_width: u32, max_height: u32) -> Self {
        self.options.max_width = Some(max_width);
        self.options.max_height = Some(max_height);
        self
    }

    pub fn with_format(mut self, format: OptimizationFormat) -> Self {
        self.options.format = Some(format);
        self
    }

    /// Optimize image
    pub async fn optimize(&self, content: &[u8], content_type: &str) -> Result<(Vec<u8>, String)> {
        // Load image
        let img = image::load_from_memory(content).map_err(|e| {
            Error::File(FileError::InvalidType(format!(
                "Failed to load image: {}",
                e
            )))
        })?;

        // Resize if needed
        let img = self.resize_if_needed(img)?;

        // Determine output format
        let format = self.determine_format(content_type)?;

        // Encode
        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);

        let output_format = self.get_output_format(format)?;

        if self.options.progressive && matches!(format, OptimizationFormat::Jpeg) {
            img.write_to(&mut cursor, output_format).map_err(|e| {
                Error::File(FileError::Upload(format!("Image encoding failed: {}", e)))
            })?;
        } else {
            img.write_to(&mut cursor, output_format).map_err(|e| {
                Error::File(FileError::Upload(format!("Image encoding failed: {}", e)))
            })?;
        }

        let content_type = self.get_content_type(format);

        Ok((buffer, content_type))
    }

    /// Create thumbnail
    pub async fn thumbnail(&self, content: &[u8], size: u32) -> Result<(Vec<u8>, String)> {
        let img = image::load_from_memory(content).map_err(|e| {
            Error::File(FileError::InvalidType(format!(
                "Failed to load image: {}",
                e
            )))
        })?;

        let thumb = img.thumbnail(size, size);

        let mut buffer = Vec::new();
        let mut cursor = Cursor::new(&mut buffer);
        thumb
            .write_to(&mut cursor, ImageOutputFormat::Jpeg(80))
            .map_err(|e| {
                Error::File(FileError::Upload(format!(
                    "Thumbnail encoding failed: {}",
                    e
                )))
            })?;

        Ok((buffer, "image/jpeg".to_string()))
    }

    /// Generate multiple sizes
    pub async fn generate_sizes(
        &self,
        content: &[u8],
        sizes: &[u32],
    ) -> Result<Vec<(u32, Vec<u8>, String)>> {
        let mut results = Vec::new();

        for &size in sizes {
            let (data, content_type) = self.thumbnail(content, size).await?;
            results.push((size, data, content_type));
        }

        Ok(results)
    }

    fn resize_if_needed(&self, img: DynamicImage) -> Result<DynamicImage> {
        let (width, height) = img.dimensions();

        let max_width = self.options.max_width.unwrap_or(u32::MAX);
        let max_height = self.options.max_height.unwrap_or(u32::MAX);

        if width <= max_width && height <= max_height {
            return Ok(img);
        }

        // Calculate new dimensions maintaining aspect ratio
        let ratio = (width as f32 / max_width as f32).max(height as f32 / max_height as f32);
        let new_width = (width as f32 / ratio).round() as u32;
        let new_height = (height as f32 / ratio).round() as u32;

        Ok(img.resize(new_width, new_height, image::imageops::FilterType::Lanczos3))
    }

    fn determine_format(&self, content_type: &str) -> Result<OptimizationFormat> {
        if let Some(format) = self.options.format {
            return Ok(format);
        }

        // Determine from content type
        Ok(match content_type {
            "image/jpeg" | "image/jpg" => OptimizationFormat::Jpeg,
            "image/png" => OptimizationFormat::Png,
            "image/webp" => OptimizationFormat::WebP,
            "image/avif" => OptimizationFormat::Avif,
            _ => OptimizationFormat::Keep,
        })
    }

    fn get_output_format(&self, format: OptimizationFormat) -> Result<ImageOutputFormat> {
        Ok(match format {
            OptimizationFormat::Jpeg => ImageOutputFormat::Jpeg(self.options.quality),
            OptimizationFormat::Png => ImageOutputFormat::Png,
            OptimizationFormat::WebP => ImageOutputFormat::WebP,
            OptimizationFormat::Avif => ImageOutputFormat::WebP, // fallback to WebP if Avif not available
            OptimizationFormat::Keep => ImageOutputFormat::Jpeg(self.options.quality), // fallback
        })
    }

    fn get_content_type(&self, format: OptimizationFormat) -> String {
        match format {
            OptimizationFormat::Jpeg => "image/jpeg",
            OptimizationFormat::Png => "image/png",
            OptimizationFormat::WebP => "image/webp",
            OptimizationFormat::Avif => "image/avif",
            OptimizationFormat::Keep => "image/jpeg",
        }
        .to_string()
    }
}

impl Default for ImageOptimizer {
    fn default() -> Self {
        Self::new(OptimizationOptions::default())
    }
}

/// Batch image processor
pub struct BatchImageProcessor {
    optimizer: ImageOptimizer,
}

impl BatchImageProcessor {
    pub fn new(optimizer: ImageOptimizer) -> Self {
        Self { optimizer }
    }

    /// Process multiple images
    pub async fn process_batch(
        &self,
        images: Vec<(Vec<u8>, String)>,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        let mut results = Vec::new();

        for (content, content_type) in images {
            let result = self.optimizer.optimize(&content, &content_type).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Generate responsive image set, actually resizing each variant.
    pub async fn generate_responsive_set(
        &self,
        content: &[u8],
        content_type: &str,
    ) -> Result<ResponsiveImageSet> {
        use image::GenericImageView;

        let sizes = [320u32, 640, 768, 1024, 1280, 1920];
        let source = image::load_from_memory(content).map_err(|e| {
            Error::Validation(ValidationError::Invalid(format!("Invalid image: {e}")))
        })?;
        let (source_w, _) = source.dimensions();

        let mut images = Vec::new();
        for &width in &sizes {
            // Never upscale: variants larger than the source reuse the source bytes.
            let (data, ct) = if width >= source_w {
                self.optimizer.optimize(content, content_type).await?
            } else {
                let resized = source.resize(width, u32::MAX, image::imageops::FilterType::Lanczos3);
                let mut out = std::io::Cursor::new(Vec::new());
                resized
                    .write_to(&mut out, output_format(content_type))
                    .map_err(|e| {
                        Error::File(FileError::Upload(format!("Failed to encode image: {e}")))
                    })?;
                (
                    out.into_inner(),
                    output_format_type(content_type).to_string(),
                )
            };
            images.push(ResponsiveImage {
                width,
                data,
                content_type: ct,
            });
        }

        Ok(ResponsiveImageSet { images })
    }
}

fn output_format(content_type: &str) -> image::ImageOutputFormat {
    match content_type {
        "image/jpeg" | "image/jpg" => image::ImageOutputFormat::Jpeg(85),
        _ => image::ImageOutputFormat::Png,
    }
}

fn output_format_type(content_type: &str) -> &'static str {
    match content_type {
        "image/jpeg" | "image/jpg" => "image/jpeg",
        _ => "image/png",
    }
}

/// Responsive image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsiveImage {
    pub width: u32,
    pub data: Vec<u8>,
    pub content_type: String,
}

/// Responsive image set
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsiveImageSet {
    pub images: Vec<ResponsiveImage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_creation() {
        let optimizer = ImageOptimizer::new(OptimizationOptions::default());
        assert_eq!(optimizer.options.quality, 85);
    }

    #[test]
    fn test_optimization_options() {
        let options = OptimizationOptions {
            quality: 90,
            max_width: Some(1920),
            max_height: Some(1080),
            format: Some(OptimizationFormat::WebP),
            strip_metadata: true,
            progressive: true,
        };

        let optimizer = ImageOptimizer::new(options);
        assert_eq!(optimizer.options.quality, 90);
        assert_eq!(optimizer.options.max_width, Some(1920));
    }

    #[tokio::test]
    async fn test_responsive_set_actually_resizes() {
        use image::GenericImageView;

        // 1000x500 red PNG.
        let img = image::RgbImage::from_pixel(1000, 500, image::Rgb([255, 0, 0]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut png, image::ImageOutputFormat::Png)
            .unwrap();
        let bytes = png.into_inner();

        let processor =
            BatchImageProcessor::new(ImageOptimizer::new(OptimizationOptions::default()));
        let set = processor
            .generate_responsive_set(&bytes, "image/png")
            .await
            .unwrap();

        for variant in &set.images {
            let decoded = image::load_from_memory(&variant.data).unwrap();
            let (width, _) = decoded.dimensions();
            // Smaller variants are truly resized; larger ones reuse the source.
            assert_eq!(width, variant.width.min(1000));
        }
    }
}
