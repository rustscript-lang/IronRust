use vm::{Program, Value};

pub fn encode_program(mut program: Program) -> Result<Vec<u8>, String> {
    for constant in &mut program.constants {
        if let Value::String(value) = constant {
            let repaired = repair_utf8_mojibake(value);
            if let std::borrow::Cow::Owned(repaired) = repaired {
                *constant = Value::string(repaired);
            }
        }
    }
    crate::wire::encode_program(&program).map_err(|error| error.to_string())
}

fn repair_utf8_mojibake(value: &str) -> std::borrow::Cow<'_, str> {
    if value.is_ascii() {
        return std::borrow::Cow::Borrowed(value);
    }

    let mut bytes = Vec::with_capacity(value.len());
    for character in value.chars() {
        let Ok(byte) = u8::try_from(u32::from(character)) else {
            return std::borrow::Cow::Borrowed(value);
        };
        bytes.push(byte);
    }

    match String::from_utf8(bytes) {
        Ok(decoded) if decoded != value => std::borrow::Cow::Owned(decoded),
        _ => std::borrow::Cow::Borrowed(value),
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_program, repair_utf8_mojibake};
    use vm::{OpCode, Program, Value};

    #[test]
    fn matches_upstream_encoder_for_normalized_programs() {
        let program = Program::new(
            vec![Value::Int(42), Value::string("plain ASCII")],
            vec![OpCode::Ret as u8],
        )
        .with_local_count(0);

        assert_eq!(
            encode_program(program.clone()).expect("bridge encoding should succeed"),
            vm::encode_program(&program).expect("upstream encoding should succeed")
        );
    }

    #[test]
    fn repairs_utf8_bytes_expanded_as_latin1_characters() {
        let mojibake = String::from_iter(['\u{f0}', '\u{9f}', '\u{98}', '\u{b5}']);
        assert_eq!(repair_utf8_mojibake(&mojibake), "😵");
    }

    #[test]
    fn preserves_text_that_is_already_unicode() {
        assert_eq!(repair_utf8_mojibake("🙂"), "🙂");
        assert_eq!(repair_utf8_mojibake("é"), "é");
        assert_eq!(repair_utf8_mojibake("plain ASCII"), "plain ASCII");
    }

    #[test]
    fn nested_constants_match_the_frozen_encoder() {
        let program = Program::new(
            vec![Value::array(vec![
                Value::Int(-42),
                Value::Bool(true),
                Value::Float(1.25),
                Value::Null,
                Value::bytes(vec![0, 127, 255]),
                Value::string("🙂"),
                Value::map(vec![(
                    Value::string("key"),
                    Value::array(vec![Value::Int(7)]),
                )]),
            ])],
            vec![OpCode::Ret as u8],
        );
        let expected = vm::encode_program(&program).expect("upstream nested constants");
        assert_eq!(
            encode_program(program).expect("compiler nested constants"),
            expected
        );
    }

    #[test]
    fn excessive_constant_nesting_matches_the_frozen_error() {
        let mut value = Value::Int(1);
        for _ in 0..65 {
            value = Value::array(vec![value]);
        }
        let program = Program::new(vec![value], vec![OpCode::Ret as u8]);
        let expected = vm::encode_program(&program)
            .expect_err("upstream depth limit")
            .to_string();
        assert_eq!(
            encode_program(program).expect_err("compiler depth limit"),
            expected
        );
    }
}
