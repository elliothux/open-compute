use super::*;
use crate::pipeline::validate_secret_set;
use open_compute_core::SecretString;

#[test]
fn standard_variables_count_utf8_text_and_canonical_json_bytes() {
    for size in [MAX_VARIABLE_BYTES - 1, MAX_VARIABLE_BYTES] {
        for text in ["x".repeat(size), "\0".repeat(size)] {
            let (values, encoded) =
                canonicalize_vars(BTreeMap::from([("TEXT".into(), serde_json::json!(text))]))
                    .unwrap();
            assert_eq!(values["TEXT"].as_str().unwrap().len(), size);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&encoded["TEXT"]).unwrap(),
                values["TEXT"]
            );
        }
        let value = serde_json::json!({"v": "x".repeat(size - 8)});
        let (_, encoded) = canonicalize_vars(BTreeMap::from([("JSON".into(), value)])).unwrap();
        assert_eq!(encoded["JSON"].len(), size);
    }
    let text = "😀".repeat(MAX_VARIABLE_BYTES / 4);
    assert!(
        canonicalize_vars(BTreeMap::from([(
            "UNICODE".into(),
            serde_json::json!(text)
        )]))
        .is_ok()
    );
    for value in [
        serde_json::json!("x".repeat(MAX_VARIABLE_BYTES + 1)),
        serde_json::json!("😀".repeat(MAX_VARIABLE_BYTES / 4) + "x"),
        serde_json::json!({"v": "x".repeat(MAX_VARIABLE_BYTES - 7)}),
    ] {
        assert_eq!(
            canonicalize_vars(BTreeMap::from([("VALUE".into(), value)]))
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimitExceeded
        );
    }
}

#[test]
fn standard_count_is_shared_by_vars_and_secrets_without_an_old_total_byte_cap() {
    for count in [MAX_VARIABLES - 1, MAX_VARIABLES] {
        for var_count in [0, count / 2, count] {
            let vars = (0..var_count)
                .map(|index| {
                    (
                        format!("VAR_{index}"),
                        serde_json::json!("v".repeat(MAX_VARIABLE_BYTES)),
                    )
                })
                .collect();
            let (vars, _) = canonicalize_vars(vars).unwrap();
            let mut secrets = (var_count..count)
                .map(|index| {
                    (
                        format!("SECRET_{index}"),
                        SecretString::new("s".repeat(MAX_VARIABLE_BYTES)),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            assert!(validate_secret_set(&secrets, &vars).is_ok());
            if count == MAX_VARIABLES {
                secrets.insert("OVERFLOW".into(), SecretString::new("s"));
                assert_eq!(
                    validate_secret_set(&secrets, &vars).unwrap_err().code(),
                    ErrorCode::SecretInvalid
                );
            }
        }
    }
    let vars = (0..=MAX_VARIABLES)
        .map(|index| (format!("VAR_{index}"), serde_json::json!("v")))
        .collect();
    assert_eq!(
        canonicalize_vars(vars).unwrap_err().code(),
        ErrorCode::ResourceLimitExceeded
    );
    let secrets = BTreeMap::from([(
        "SECRET".into(),
        SecretString::new("s".repeat(MAX_VARIABLE_BYTES + 1)),
    )]);
    assert_eq!(
        validate_secret_set(&secrets, &BTreeMap::new())
            .unwrap_err()
            .code(),
        ErrorCode::SecretInvalid
    );
}
