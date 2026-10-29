//! Admission of the pinned metadata declarations, not equivalence to upstream pixel processing.
use crate::{Result, fail, models};
use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct Metadata {
    pub dictionary: Vec<String>,
    pub classes: Option<usize>,
}

pub fn validate_metadata(id: &str, text: &str) -> Result<Metadata> {
    let spec = models::specs()?
        .into_iter()
        .find(|spec| spec.id == id)
        .ok_or_else(|| fail("Unknown model metadata identifier"))?;
    if text.len() > 262_144 {
        return Err(fail("Model metadata exceeds its byte bound"));
    }
    let hash: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if hash != spec.metadata_sha256 {
        return Err(fail("Model metadata checksum mismatch"));
    }
    let value = serde_yaml_ng::from_str(text).map_err(|_| fail("Invalid model metadata YAML"))?;
    validate_profile(id, &value)
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(Value::String(key.into())))
        .ok_or_else(|| fail(format!("Missing model metadata field: {key}")))
}

fn string(value: &Value, expected: &str) -> Result<()> {
    if value.as_str() != Some(expected) {
        return Err(fail(format!(
            "Unsupported model metadata declaration: {expected}"
        )));
    }
    Ok(())
}

fn number(value: &Value, expected: f64) -> Result<()> {
    if value
        .as_f64()
        .is_none_or(|actual| !actual.is_finite() || (actual - expected).abs() > 1e-9)
    {
        return Err(fail("Unsupported model metadata numeric declaration"));
    }
    Ok(())
}

fn vector(value: &Value, expected: &[f64]) -> Result<()> {
    let values = value
        .as_sequence()
        .filter(|values| values.len() == expected.len())
        .ok_or_else(|| fail("Invalid model metadata vector"))?;
    for (value, expected) in values.iter().zip(expected) {
        number(value, *expected)?;
    }
    Ok(())
}

fn inference_keys(value: &Value, allowed: &[&str]) -> Result<()> {
    let mapping = value
        .as_mapping()
        .ok_or_else(|| fail("Inference transform configuration must be a mapping"))?;
    if mapping
        .keys()
        .any(|key| key.as_str().is_none_or(|key| !allowed.contains(&key)))
    {
        return Err(fail("Unsupported inference transform parameter"));
    }
    Ok(())
}

fn validate_profile(id: &str, value: &Value) -> Result<Metadata> {
    let (model_name, characters) = match id {
        "det" => ("PP-OCRv5_mobile_det", None),
        "fa" => ("arabic_PP-OCRv5_mobile_rec", Some(747)),
        "en-ja" => ("PP-OCRv5_mobile_rec", Some(18_383)),
        "ja-specialist" => ("japan_PP-OCRv3_mobile_rec", Some(4399)),
        _ => return Err(fail("Unsupported model metadata profile")),
    };
    string(field(field(value, "Global")?, "model_name")?, model_name)?;
    let operations = field(field(value, "PreProcess")?, "transform_ops")?
        .as_sequence()
        .filter(|ops| !ops.is_empty() && ops.len() <= 16)
        .ok_or_else(|| fail("Invalid model preprocessing operation inventory"))?;
    let expected: &[&str] = if characters.is_none() {
        &[
            "DecodeImage",
            "DetResizeForTest",
            "NormalizeImage",
            "ToCHWImage",
        ]
    } else {
        &["DecodeImage", "RecResizeImg"]
    };
    let mut next = 0;
    for operation in operations {
        let mapping = operation
            .as_mapping()
            .filter(|mapping| mapping.len() == 1)
            .ok_or_else(|| fail("Each model transform needs exactly one declaration"))?;
        let (name, config) = mapping
            .iter()
            .next()
            .ok_or_else(|| fail("Missing model transform declaration"))?;
        let name = name
            .as_str()
            .ok_or_else(|| fail("Invalid model transform name"))?;
        // Label encoding and result-key selection do not transform inference pixels.
        if name == "KeepKeys"
            || (characters.is_none() && name == "DetLabelEncode")
            || (characters.is_some() && name == "MultiLabelEncode")
        {
            continue;
        }
        if expected.get(next).copied() != Some(name) {
            return Err(fail(
                "Unsupported, repeated or reordered inference transform",
            ));
        }
        next += 1;
        match name {
            "DecodeImage" => {
                inference_keys(config, &["img_mode", "channel_first"])?;
                string(field(config, "img_mode")?, "BGR")?;
                if field(config, "channel_first")?.as_bool() != Some(false) {
                    return Err(fail("Model decoder must declare HWC channel order"));
                }
            }
            "DetResizeForTest" => {
                inference_keys(config, &["resize_long"])?;
                number(field(config, "resize_long")?, 960.0)?;
            }
            "NormalizeImage" => {
                inference_keys(config, &["order", "scale", "mean", "std"])?;
                string(field(config, "order")?, "hwc")?;
                string(field(config, "scale")?, "1./255.")?;
                vector(field(config, "mean")?, &[0.485, 0.456, 0.406])?;
                vector(field(config, "std")?, &[0.229, 0.224, 0.225])?;
            }
            "ToCHWImage" => {
                if !config.is_null() {
                    return Err(fail("Unsupported CHW transform configuration"));
                }
            }
            "RecResizeImg" => {
                inference_keys(config, &["image_shape"])?;
                vector(field(config, "image_shape")?, &[3.0, 48.0, 320.0])?;
            }
            _ => return Err(fail("Unsupported inference transform")),
        }
    }
    if next != expected.len() {
        return Err(fail("Missing required inference transform"));
    }
    let post = field(value, "PostProcess")?;
    if let Some(characters) = characters {
        inference_keys(post, &["name", "character_dict"])?;
        string(field(post, "name")?, "CTCLabelDecode")?;
        let entries = field(post, "character_dict")?
            .as_sequence()
            .filter(|entries| entries.len() == characters)
            .ok_or_else(|| fail("Recognition metadata dictionary count mismatch"))?;
        let mut dictionary = Vec::with_capacity(characters + 2);
        dictionary.push(String::new());
        for entry in entries {
            let token = entry
                .as_str()
                .filter(|token| !token.is_empty() && token.len() <= 256 && !token.contains('\0'))
                .ok_or_else(|| fail("Invalid recognition dictionary entry"))?;
            // Ordering and duplicate tokens are meaningful model indices, including existing spaces.
            dictionary.push(token.into());
        }
        dictionary.push(" ".into());
        Ok(Metadata {
            classes: Some(dictionary.len()),
            dictionary,
        })
    } else {
        inference_keys(
            post,
            &[
                "name",
                "thresh",
                "box_thresh",
                "max_candidates",
                "unclip_ratio",
            ],
        )?;
        string(field(post, "name")?, "DBPostProcess")?;
        for (key, expected) in [
            ("thresh", 0.3),
            ("box_thresh", 0.6),
            ("max_candidates", 1000.0),
            ("unclip_ratio", 1.5),
        ] {
            number(field(post, key)?, expected)?;
        }
        Ok(Metadata {
            dictionary: Vec::new(),
            classes: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Bypass only the external hash check so mutations exercise the actual profile guards.
    fn profile(id: &str, text: &str) -> Result<Metadata> {
        let value = serde_yaml_ng::from_str(text).map_err(|_| fail("Invalid fixture YAML"))?;
        validate_profile(id, &value)
    }

    #[test]
    fn detector_profile_rejects_changed_pixel_and_postprocess_declarations() {
        let original = models::metadata("det").unwrap();
        for (from, to) in [
            ("PP-OCRv5_mobile_det", "other-det"),
            ("img_mode: BGR", "img_mode: RGB"),
            ("channel_first: false", "channel_first: true"),
            ("resize_long: 960", "resize_long: 736"),
            (
                "resize_long: 960",
                "resize_long: 960\n      limit_side_len: 736",
            ),
            ("order: hwc", "order: chw"),
            ("scale: 1./255.", "scale: 1./128."),
            ("- 0.485", "- 0.5"),
            ("- 0.229", "- 0.25"),
            ("name: DBPostProcess", "name: OtherPostProcess"),
            ("thresh: 0.3", "thresh: 0.4"),
            ("box_thresh: 0.6", "box_thresh: 0.7"),
            ("max_candidates: 1000", "max_candidates: 999"),
            ("unclip_ratio: 1.5", "unclip_ratio: 2.0"),
            ("  - ToCHWImage: null\n", ""),
            (
                "  - ToCHWImage: null",
                "  - ToCHWImage: null\n  - ToCHWImage: null",
            ),
            ("  - ToCHWImage: null", "  - UnknownPixelTransform: null"),
        ] {
            assert!(original.contains(from), "mutation fixture absent: {from}");
            assert!(
                profile("det", &original.replace(from, to)).is_err(),
                "accepted {from}"
            );
        }
    }

    #[test]
    fn recognizer_profile_rejects_shape_decoder_dictionary_and_order_changes() {
        let original = models::metadata("fa").unwrap();
        for (from, to) in [
            ("      - 48\n", "      - 32\n"),
            ("name: CTCLabelDecode", "name: AttentionDecode"),
            ("  - '!'\n", ""),
            ("  - '!'", "  - ''"),
            ("  - RecResizeImg:", "  - OtherResize:"),
        ] {
            assert!(original.contains(from), "mutation fixture absent: {from}");
            assert!(
                profile("fa", &original.replace(from, to)).is_err(),
                "accepted {from}"
            );
        }
        let reordered = original.replace(
            "  - DecodeImage:\n      channel_first: false\n      img_mode: BGR\n",
            "",
        );
        assert!(profile("fa", &reordered).is_err());
    }

    #[test]
    fn non_inference_tuning_and_label_declarations_are_not_pixel_policy() {
        let original = models::metadata("fa").unwrap();
        let changed = original
            .replace("gtc_encode: NRTRLabelEncode", "gtc_encode: TrainingOnly")
            .replace("      - 160\n", "      - 192\n");
        assert_eq!(profile("fa", &changed).unwrap().classes, Some(749));
    }
}
