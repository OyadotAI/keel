//! Bounded PNG results stay images across the desktop/Oya boundary instead of entering text truncation.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const SCREENSHOT_WIRE_BYTES: usize = 8 * 1024 * 1024;
const IMAGE_BYTES: usize = 6 * 1024 * 1024;
const PNG_HEADER_BYTES: usize = 24;
const MAX_PIXELS: u64 = 16_777_216;

/// A native PNG, never a path or executable data URL supplied by the page.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub data: String,
    pub mime_type: String,
}

impl Image {
    /// Check encoding and actual PNG dimensions before giving a client untrusted pixels.
    pub fn dimensions(&self) -> Result<(u32, u32), String> {
        if self.mime_type != "image/png" || self.data.len() > IMAGE_BYTES {
            return Err(
                "Screenshot must be a PNG smaller than 6 MiB. Use a smaller viewport.".into(),
            );
        }
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| "Invalid screenshot encoding.")?;
        if bytes.len() < PNG_HEADER_BYTES
            || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
            || &bytes[12..16] != b"IHDR"
        {
            return Err("The browser returned an invalid PNG screenshot.".into());
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err("Screenshot exceeds 16,777,216 pixels. Use a smaller viewport.".into());
        }
        Ok((width, height))
    }
}

/// Text remains compatible with old desktop senders; captures add exactly one validated image.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Output {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
}

impl Output {
    /// Keep ordinary browser operations textual and subject to the existing text budget.
    pub fn text(text: String) -> Self {
        Self { text, image: None }
    }

    /// Only a screenshot request may return pixels; never silently truncate a capture.
    pub(super) fn bounded(mut self, screenshot: bool) -> Result<Self, String> {
        self.text = super::capped(self.text);
        match (&self.image, screenshot) {
            (Some(image), true) => { image.dimensions()?; }
            (None, true) => return Err("The browser returned no screenshot. Update the desktop app or Oya and retry browser_screenshot.".into()),
            (Some(_), false) => return Err("Unexpected image from a text browser operation.".into()),
            (None, false) => {}
        }
        Ok(self)
    }

    /// MCP clients receive image content, never base64 buried in text.
    pub(crate) fn content(self) -> Vec<Value> {
        let mut blocks = vec![json!({"type":"text", "text":self.text})];
        if let Some(image) = self.image {
            blocks.push(json!({"type":"image", "data":image.data, "mimeType":image.mime_type}));
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
    #[test]
    fn a_capture_survives_as_mcp_image_content() {
        let image = Image {
            data: PNG.into(),
            mime_type: "image/png".into(),
        };
        assert_eq!(image.dimensions().unwrap(), (1, 1));
        let output = Output {
            text: "Screenshot".into(),
            image: Some(image),
        }
        .bounded(true)
        .unwrap()
        .content();
        assert_eq!(output[1]["type"], "image");
        assert_eq!(output[1]["data"], PNG);
    }
    #[test]
    fn invalid_missing_oversized_and_unsolicited_images_are_refused() {
        assert!(Output::default().bounded(true).is_err());
        for data in ["bad".into(), "a".repeat(IMAGE_BYTES + 1)] {
            assert!(
                Image {
                    data,
                    mime_type: "image/png".into()
                }
                .dimensions()
                .is_err()
            );
        }
        let mut image = Image {
            data: PNG.into(),
            mime_type: "image/svg+xml".into(),
        };
        assert!(image.dimensions().is_err());
        image.mime_type = "image/png".into();
        assert!(
            Output {
                text: String::new(),
                image: Some(image)
            }
            .bounded(false)
            .is_err()
        );
        let mut bytes = STANDARD.decode(PNG).unwrap();
        bytes[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(
            Image {
                data: STANDARD.encode(bytes),
                mime_type: "image/png".into()
            }
            .dimensions()
            .is_err()
        );
    }
}
