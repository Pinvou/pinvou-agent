use base64::{Engine, engine::general_purpose::STANDARD};
use pinvou_upgrade_codec::{
    CanonicalAppVersion, Document, Error, InputKind, VersionError, compare_app_versions,
};
use serde_json::{Value, json};
use std::{
    cmp::Ordering,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn wire(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn shape(bytes: &[u8]) -> Result<CanonicalAppVersion, VersionError> {
    Document::parse(bytes, InputKind::Metadata)
        .map_err(VersionError::Json)?
        .app_version_shape()
}
fn version(text: &str) -> CanonicalAppVersion {
    shape(&wire(&json!(text))).unwrap()
}
#[test]
fn decimal_triples_compare_each_component_and_beyond_native_integer_magnitudes() {
    for (a, b) in [
        ("9.0.0", "10.0.0"),
        ("99.0.0", "100.0.0"),
        ("1.9.99", "1.10.0"),
        ("1.1.9", "1.1.10"),
        ("9007199254740991.0.0", "9007199254740992.0.0"),
        ("18446744073709551615.0.0", "18446744073709551616.0.0"),
        (
            "999999999999999999999999999999999999999.0.0",
            "1000000000000000000000000000000000000000.0.0",
        ),
    ] {
        let a = version(a);
        let b = version(b);
        assert_eq!(compare_app_versions(&a, &b), Ordering::Less);
        assert_eq!(compare_app_versions(&b, &a), Ordering::Greater);
        assert_eq!(compare_app_versions(&a, &a), Ordering::Equal);
    }
    let versions: Vec<_> = [
        "0.0.0", "0.0.1", "0.1.0", "1.0.0", "9.0.0", "10.0.0", "100.0.0",
    ]
    .into_iter()
    .map(version)
    .collect();
    for i in 0..versions.len() {
        for j in i..versions.len() {
            for k in j..versions.len() {
                assert_ne!(
                    compare_app_versions(&versions[i], &versions[j]),
                    Ordering::Greater
                );
                assert_ne!(
                    compare_app_versions(&versions[j], &versions[k]),
                    Ordering::Greater
                );
                assert_ne!(
                    compare_app_versions(&versions[i], &versions[k]),
                    Ordering::Greater
                );
            }
        }
    }
    let escaped = shape(br#""\u0031.\u0032.\u0033""#).unwrap();
    assert_eq!(escaped.as_str(), "1.2.3");
    assert_eq!(
        compare_app_versions(&escaped, &version("1.2.3")),
        Ordering::Equal
    );
}
fn invalid() -> Vec<Value> {
    let mut values: Vec<_> = [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "01.2.3",
        "1.02.3",
        "1.2.03",
        "+1.2.3",
        "-1.2.3",
        "-0.0.0",
        "1e2.0.0",
        "1.2.3-beta",
        "v1.2.3",
        " 1.2.3",
        "1.2.3 ",
        "1..3",
        "1.2.",
        ".2.3",
        "1.2.3\n",
        "1.2.3\r",
        "1.2.3\r\n",
        "1.2.3\u{2028}",
        "1.2.3\u{2029}",
        "\u{0661}.2.3",
        "1\u{ff0e}2.3",
        "1.2.3\0",
    ]
    .into_iter()
    .map(|text| json!(text))
    .collect();
    values.extend([
        Value::Null,
        json!(false),
        json!(1),
        json!(1.2),
        json!([]),
        json!(["1.2.3"]),
        json!({"version":"1.2.3"}),
    ]);
    values
}
#[test]
fn grammar_rejects_noncanonical_inputs_without_trimming_or_semver_extensions() {
    for value in invalid() {
        assert!(matches!(
            shape(&wire(&value)),
            Err(VersionError::InvalidVersion)
        ));
    }
    assert!(matches!(
        shape(br#"{"version":"1.2.3","ver\u0073ion":"1.2.3"}"#),
        Err(VersionError::Json(Error::DuplicateKey))
    ));
    let invalid_utf8 = [b'"', 0xff, b'"'];
    assert!(matches!(
        shape(&invalid_utf8),
        Err(VersionError::Json(Error::InvalidUtf8))
    ));
}
#[test]
fn decoded_sixty_four_kib_boundary_keeps_original_metadata_and_credential_limits() {
    let maximum = format!("{}.0.0", "9".repeat(65_532));
    let bytes = wire(&json!(maximum));
    assert_eq!(shape(&bytes).unwrap().as_str().len(), 65_536);
    assert!(matches!(
        Document::parse(&bytes, InputKind::Credential),
        Err(Error::InputLimit)
    ));
    let excess = format!("{}.0.0", "9".repeat(65_533));
    assert!(matches!(
        shape(&wire(&json!(excess))),
        Err(VersionError::Json(Error::StringLimit))
    ));
    for index in 0..3 {
        let mut parts = vec!["0".to_owned(); 3];
        parts[index] = "9".repeat(65_532);
        let maximum = parts.join(".");
        let bytes = wire(&json!(maximum));
        assert_eq!(shape(&bytes).unwrap().as_str().len(), 65_536);
        parts[index] = format!("{}8", "9".repeat(65_531));
        let smaller = parts.join(".");
        assert_eq!(
            compare_app_versions(&version(&smaller), &version(&maximum)),
            Ordering::Less
        );
        parts[index] = "9".repeat(65_533);
        assert!(matches!(
            shape(&wire(&json!(parts.join(".")))),
            Err(VersionError::Json(Error::StringLimit))
        ));
    }
    let smaller = format!("{}8.0.0", "9".repeat(65_531));
    assert_eq!(
        compare_app_versions(&version(&smaller), &version(&maximum)),
        Ordering::Less
    );
}
#[test]
fn actual_original_node_bigint_comparison_matches_both_operand_shapes_and_boundaries() {
    let good: Vec<Value> = [
        "0.0.0",
        "0.0.1",
        "0.1.0",
        "1.0.0",
        "9.0.0",
        "10.0.0",
        "99.0.0",
        "100.0.0",
        "1.1.9",
        "1.1.10",
        "1.9.0",
        "1.10.0",
        "9007199254740991.0.0",
        "9007199254740992.0.0",
        "18446744073709551615.0.0",
        "18446744073709551616.0.0",
    ]
    .into_iter()
    .map(|text| json!(text))
    .collect();
    let mut cases = vec![];
    for a in &good {
        for b in &good {
            cases.push((wire(a), wire(b)));
        }
    }
    for bad in invalid() {
        cases.push((wire(&bad), wire(&json!("1.2.3"))));
        cases.push((wire(&json!("1.2.3")), wire(&bad)));
    }
    let maximum = wire(&json!(format!("{}.0.0", "9".repeat(65_532))));
    let smaller = wire(&json!(format!("{}8.0.0", "9".repeat(65_531))));
    let excess = wire(&json!(format!("{}.0.0", "9".repeat(65_533))));
    for (a, b) in [
        (maximum.clone(), maximum.clone()),
        (maximum.clone(), smaller.clone()),
        (smaller.clone(), maximum.clone()),
        (maximum.clone(), wire(&json!("100.0.0"))),
        (excess.clone(), maximum.clone()),
        (maximum, excess),
    ] {
        cases.push((a, b));
    }
    for raw in [
        br#""\u0031.\u0032.\u0033""#.to_vec(),
        b" \"1.2.3\"\r\n".to_vec(),
        br#"{"version":"1.2.3","ver\u0073ion":"1.2.3"}"#.to_vec(),
        vec![b'"', 0xff, b'"'],
    ] {
        cases.push((raw.clone(), wire(&json!("1.2.3"))));
        cases.push((wire(&json!("1.2.3")), raw));
    }
    for index in 1..3 {
        let mut parts = vec!["0".to_owned(); 3];
        parts[index] = "9".repeat(65_532);
        let maximum = wire(&json!(parts.join(".")));
        parts[index] = format!("{}8", "9".repeat(65_531));
        let smaller = wire(&json!(parts.join(".")));
        parts[index] = "9".repeat(65_533);
        let excess = wire(&json!(parts.join(".")));
        for (a, b) in [
            (maximum.clone(), maximum.clone()),
            (maximum.clone(), smaller.clone()),
            (smaller, maximum.clone()),
            (excess.clone(), maximum.clone()),
            (maximum, excess),
        ] {
            cases.push((a, b));
        }
    }
    let giant_later = wire(&json!(format!(
        "0.{}.{}",
        "9".repeat(32765),
        "9".repeat(32765)
    )));
    cases.push((giant_later.clone(), wire(&json!("1.0.0"))));
    cases.push((wire(&json!("1.0.0")), giant_later));
    let samples: Vec<_> = cases
        .iter()
        .map(|(a, b)| json!([STANDARD.encode(a), STANDARD.encode(b)]))
        .collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|(a, b)| match (shape(a), shape(b)) {
            (Ok(a), Ok(b)) => json!(match compare_app_versions(&a, &b) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            }),
            _ => Value::Null,
        })
        .collect();
    let reference = root()
        .join("specs/upgrade/v1/reference/semantics.mjs")
        .to_string_lossy()
        .into_owned();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("version-oracle.mjs"),
            &reference,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&samples).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original version oracle unavailable"
    );
    let actual: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            actual == expected,
            "version oracle mismatch at case {index}"
        );
    }
    eprintln!(
        "{} original Node BigInt version pairs passed",
        expected.len()
    );
}
