//! Validate viewport requests before a browser or permission prompt can produce an effect.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const MAX_DIMENSION: u32 = 4096;
const MIN_DPR: f64 = 0.25;
const MAX_DPR: f64 = 4.0;
const MAX_PIXELS: f64 = 16_777_216.0;

/// A complete size or an explicit reset; omission of optional settings preserves the lane's value.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Viewport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_scale_factor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobile: Option<bool>,
    #[serde(default)]
    pub reset: bool,
}

impl Viewport {
    /// Strictly parse the tool boundary while allowing its existing human description.
    pub(super) fn parse(args: &Value) -> Result<Self, String> {
        let mut fields = args
            .as_object()
            .ok_or("Viewport arguments must be an object.")?
            .clone();
        fields.remove("description");
        let settings: Self = serde_json::from_value(Value::Object(fields)).map_err(|_| "Invalid viewport fields. Supply width/height, optional deviceScaleFactor/mobile, or reset:true.")?;
        settings.validate()?;
        Ok(settings)
    }
    fn validate(&self) -> Result<(), String> {
        if self.reset {
            return if self.width.is_none()
                && self.height.is_none()
                && self.device_scale_factor.is_none()
                && self.mobile.is_none()
            {
                Ok(())
            } else {
                Err("reset:true cannot be combined with viewport settings.".into())
            };
        }
        let (Some(width), Some(height)) = (self.width, self.height) else {
            return Err("width and height are required together, or use reset:true.".into());
        };
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err("width and height must be whole numbers from 1 to 4096 CSS pixels.".into());
        }
        let dpr = self.device_scale_factor.unwrap_or(1.0);
        if !dpr.is_finite() || !(MIN_DPR..=MAX_DPR).contains(&dpr) {
            return Err("deviceScaleFactor must be between 0.25 and 4.".into());
        }
        if f64::from(width) * f64::from(height) * dpr * dpr > MAX_PIXELS {
            return Err(
                "Viewport exceeds 16,777,216 physical pixels. Use a smaller viewport or DPR."
                    .into(),
            );
        }
        Ok(())
    }
}

/// The schema names supported values; adapters still validate their merged settings before effects.
pub(super) fn properties() -> Map<String, Value> {
    json!({
        "width":{"type":"integer","minimum":1,"maximum":MAX_DIMENSION},
        "height":{"type":"integer","minimum":1,"maximum":MAX_DIMENSION},
        "deviceScaleFactor":{"type":"number","minimum":MIN_DPR,"maximum":MAX_DPR},
        "mobile":{"type":"boolean"}, "reset":{"type":"boolean"}
    })
    .as_object()
    .unwrap()
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dimensions_reset_and_optional_settings_are_checked_before_effects() {
        for width in [375, 768, 1280] {
            assert!(Viewport::parse(&json!({"width":width,"height":812})).is_ok());
        }
        assert!(Viewport::parse(&json!({"reset":true})).is_ok());
        for bad in [
            json!({}),
            json!({"width":375}),
            json!({"width":0,"height":812}),
            json!({"width":1.5,"height":812}),
            json!({"width":4097,"height":812}),
            json!({"width":375,"height":812,"deviceScaleFactor":0}),
            json!({"reset":true,"mobile":false}),
            json!({"width":4096,"height":4096,"deviceScaleFactor":4}),
            json!({"reset":true,"surprise":1}),
        ] {
            assert!(Viewport::parse(&bad).is_err(), "{bad}");
        }
    }
}
