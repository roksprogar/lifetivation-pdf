//! Images, loaded on demand through the caller's loader: once per reference, only for images the
//! document actually uses, shared by layout (sizes) and painting (pixels).

use std::collections::HashMap;
use std::sync::Arc;

use crate::Error;

pub(crate) struct LoadedImage {
    pub bytes: Arc<Vec<u8>>,
    /// Intrinsic size in CSS px.
    pub width: u32,
    pub height: u32,
}

pub(crate) struct Images<'a> {
    load: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>,
    loaded: HashMap<String, Arc<LoadedImage>>,
}

impl<'a> Images<'a> {
    pub fn new(load: &'a mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Self {
        Self {
            load,
            loaded: HashMap::new(),
        }
    }

    pub fn get(&mut self, src: &str) -> Result<Arc<LoadedImage>, Error> {
        if let Some(image) = self.loaded.get(src) {
            return Ok(image.clone());
        }
        let bytes = (self.load)(src).ok_or_else(|| Error::MissingImage(src.to_string()))?;
        let (width, height) =
            png_size(&bytes).ok_or_else(|| Error::InvalidImage(src.to_string()))?;
        let image = Arc::new(LoadedImage {
            bytes: Arc::new(bytes),
            width,
            height,
        });
        self.loaded.insert(src.to_string(), image.clone());
        Ok(image)
    }
}

/// Intrinsic size in pixels of a PNG, from its header, without decoding it.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

/// A PNG header claiming `width` × `height`: enough for layout, which never decodes pixels.
#[cfg(test)]
pub(crate) fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_each_reference_once_and_reads_png_sizes() {
        let mut calls = Vec::new();
        let mut load = |src: &str| {
            calls.push(src.to_string());
            match src {
                "a.png" => Some(png_header(800, 281)),
                "b.gif" => Some(b"GIF89a................".to_vec()),
                _ => None,
            }
        };
        let mut images = Images::new(&mut load);
        let a = images.get("a.png").unwrap();
        assert_eq!((a.width, a.height), (800, 281));
        images.get("a.png").unwrap();
        assert!(matches!(images.get("b.gif"), Err(Error::InvalidImage(src)) if src == "b.gif"));
        assert!(matches!(images.get("c.png"), Err(Error::MissingImage(src)) if src == "c.png"));
        drop(images);
        assert_eq!(calls, ["a.png", "b.gif", "c.png"]);
    }
}
