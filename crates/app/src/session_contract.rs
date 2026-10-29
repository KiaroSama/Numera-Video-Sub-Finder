use crate::{Result, fail};
use ort::{
    session::Session,
    value::{TensorElementType, ValueType},
};

pub(crate) fn validate(session: &Session, classes: Option<usize>) -> Result<()> {
    let inputs = session.inputs();
    let outputs = session.outputs();
    if inputs.len() != 1 || outputs.len() != 1 {
        return Err(fail(
            "OCR session requires exactly one input and one output",
        ));
    }
    validate_types(
        (inputs[0].name(), inputs[0].dtype()),
        (outputs[0].name(), outputs[0].dtype()),
        classes,
    )
}

fn tensor<'a>(name: &str, dtype: &'a ValueType, rank: usize) -> Result<&'a [i64]> {
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err(fail("OCR session has an invalid tensor name"));
    }
    match dtype {
        ValueType::Tensor { ty, shape, .. }
            if *ty == TensorElementType::Float32
                && shape.len() == rank
                && shape.iter().all(|&d| d == -1 || d > 0) =>
        {
            Ok(shape)
        }
        _ => Err(fail(
            "OCR session tensor type, rank or dimensions are incompatible",
        )),
    }
}

fn validate_types(
    input: (&str, &ValueType),
    output: (&str, &ValueType),
    classes: Option<usize>,
) -> Result<()> {
    let input = tensor(input.0, input.1, 4)?;
    let output = tensor(output.0, output.1, if classes.is_some() { 3 } else { 4 })?;
    if !matches!(input[0], -1 | 1) || input[1] != 3 || !matches!(output[0], -1 | 1) {
        return Err(fail("OCR session requires single-batch NCHW image tensors"));
    }
    if let Some(classes) = classes {
        if input[2] != 48 || input[3] != -1 || output[1] != -1 || output[2] != classes as i64 {
            return Err(fail(
                "Recognizer height, variable width/time or vocabulary does not match its contract",
            ));
        }
    } else if input[2..] != [-1, -1] || !matches!(output[1], -1 | 1) || output[2..] != [-1, -1] {
        // The pinned detector declares symbolic output channels; run-time maps still require one.
        return Err(fail(
            "Detector requires variable spatial axes and a compatible probability-map channel",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ort::value::{Shape, SymbolicDimensions};

    fn value(shape: &[i64]) -> ValueType {
        ValueType::Tensor {
            ty: TensorElementType::Float32,
            shape: Shape::new(shape.iter().copied()),
            dimension_symbols: SymbolicDimensions::new(vec![String::new(); shape.len()]),
        }
    }

    #[test]
    fn detector_and_recognizer_signatures_are_checked_before_use() {
        assert!(
            validate_types(
                ("image", &value(&[-1, 3, -1, -1])),
                ("maps", &value(&[-1, 1, -1, -1])),
                None
            )
            .is_ok()
        );
        assert!(
            validate_types(
                ("image", &value(&[1, 3, 48, -1])),
                ("ctc", &value(&[1, -1, 4401])),
                Some(4401)
            )
            .is_ok()
        );
        for shape in [
            vec![1, 1, 48, 320],
            vec![2, 3, 48, 320],
            vec![1, 3, 32, 320],
            vec![1, 3, 48, 0],
            vec![1, 3, 48, -2],
            vec![1, 3, 48, 64],
            vec![1, 3, 48, 320],
        ] {
            assert!(
                validate_types(
                    ("image", &value(&shape)),
                    ("ctc", &value(&[1, -1, 4401])),
                    Some(4401)
                )
                .is_err()
            );
        }
        for shape in [
            vec![1, -1, 4400],
            vec![1, -1, -1],
            vec![2, -1, 4401],
            vec![1, 4401],
            vec![1, 10, 4401],
        ] {
            assert!(
                validate_types(
                    ("image", &value(&[1, 3, 48, -1])),
                    ("ctc", &value(&shape)),
                    Some(4401)
                )
                .is_err()
            );
        }
        assert!(
            validate_types(
                ("image", &value(&[1, 3, -1, -1])),
                ("maps", &value(&[1, -1, -1, -1])),
                None
            )
            .is_ok()
        );
        for shape in [vec![1, 3, 64, 64], vec![1, 3, -1, 960], vec![1, 3, 960, -1]] {
            assert!(
                validate_types(
                    ("image", &value(&shape)),
                    ("maps", &value(&[1, -1, -1, -1])),
                    None
                )
                .is_err()
            );
        }
        assert!(
            validate_types(
                ("image", &value(&[1, 3, -1, -1])),
                ("maps", &value(&[1, 2, -1, -1])),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_names_and_non_float_tensors_are_rejected() {
        for name in ["", "line\nbreak"] {
            assert!(
                validate_types(
                    (name, &value(&[1, 3, -1, -1])),
                    ("maps", &value(&[1, 1, -1, -1])),
                    None
                )
                .is_err()
            );
        }
        let mut input = value(&[1, 3, -1, -1]);
        if let ValueType::Tensor { ty, .. } = &mut input {
            *ty = TensorElementType::Int64;
        }
        assert!(
            validate_types(("image", &input), ("maps", &value(&[1, 1, -1, -1])), None).is_err()
        );
    }
}
