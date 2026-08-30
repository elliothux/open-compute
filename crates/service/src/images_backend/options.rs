//! Strict parsing and compilation of the supported Images operation subset.

use super::option;
use open_compute_core::PlatformError;
use open_compute_images::{Gravity, ImageOperation, ResizeFit, ResizeOperation};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TransformRequest {
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<ResizeFit>,
    gravity: Option<Gravity>,
    rotate: Option<u16>,
    flip: Option<String>,
    background: Option<String>,
    blur: Option<f32>,
}

impl TransformRequest {
    pub(super) fn operations(self) -> Result<Vec<ImageOperation>, PlatformError> {
        let mut result = Vec::new();
        if self.width.is_some() || self.height.is_some() {
            result.push(ImageOperation::Resize(ResizeOperation {
                width: self.width,
                height: self.height,
                fit: self.fit.unwrap_or_default(),
                gravity: self.gravity.unwrap_or_default(),
                background: self
                    .background
                    .as_deref()
                    .map(parse_color)
                    .transpose()?
                    .unwrap_or([0, 0, 0, 0]),
            }));
        } else if self.fit.is_some() || self.gravity.is_some() {
            return Err(option());
        }
        if let Some(degrees) = self.rotate {
            result.push(ImageOperation::Rotate { degrees });
        }
        if let Some(flip) = self.flip {
            let (horizontal, vertical) = match flip.as_str() {
                "horizontal" => (true, false),
                "vertical" => (false, true),
                "both" => (true, true),
                _ => return Err(option()),
            };
            result.push(ImageOperation::Flip {
                horizontal,
                vertical,
            });
        }
        if self.width.is_none()
            && let Some(background) = self.background
        {
            result.push(ImageOperation::Background {
                rgba: parse_color(&background)?,
            });
        }
        if let Some(sigma) = self.blur {
            result.push(ImageOperation::Blur { sigma });
        }
        if result.is_empty() {
            return Err(option());
        }
        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DrawRequest {
    #[serde(default)]
    pub(super) left: i32,
    #[serde(default)]
    pub(super) top: i32,
    #[serde(default = "one")]
    pub(super) opacity: f32,
    #[serde(default)]
    repeat: bool,
    #[serde(default)]
    blend: Option<String>,
}

impl DrawRequest {
    pub(super) fn validate(&self) -> Result<(), PlatformError> {
        if self.repeat
            || !self.opacity.is_finite()
            || !(0.0..=1.0).contains(&self.opacity)
            || self
                .blend
                .as_deref()
                .is_some_and(|value| !matches!(value, "normal" | "over"))
        {
            Err(option())
        } else {
            Ok(())
        }
    }
}

fn parse_color(value: &str) -> Result<[u8; 4], PlatformError> {
    let value = value.strip_prefix('#').ok_or_else(option)?;
    let bytes = hex::decode(value).map_err(|_| option())?;
    match bytes.as_slice() {
        [r, g, b] => Ok([*r, *g, *b, 255]),
        [r, g, b, a] => Ok([*r, *g, *b, *a]),
        _ => Err(option()),
    }
}

fn one() -> f32 {
    1.0
}
