use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pinvou_upgrade_codec::{Document, Error, InputKind};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn canonical(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    Document::parse(bytes, InputKind::Metadata)?.canonical_bytes()
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(7)
        .unwrap()
        .to_owned()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn original_twenty_frozen_envelope_and_signed_body_bytes() {
    let vectors: serde_json::Value = serde_json::from_slice(
        &fs::read(root().join("specs/upgrade/v1/vectors/crypto/envelopes.json")).unwrap(),
    )
    .unwrap();
    let vectors = vectors["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 20);
    for vector in vectors {
        let wire = URL_SAFE_NO_PAD
            .decode(vector["envelopeBase64Url"].as_str().unwrap())
            .unwrap();
        let envelope = Document::parse(&wire, InputKind::Metadata).unwrap();
        let bytes = envelope.canonical_bytes().unwrap();
        assert_eq!(
            hex(&Sha256::digest(&bytes)),
            vector["expectedEnvelopeSha256"].as_str().unwrap()
        );
        let signed = &envelope.value()["signed"];
        let wrapper = serde_json::json!({
            "protocolVersion":signed["protocolVersion"], "role":signed["role"],
            "product":signed["product"], "component":signed["component"],
            "scope":signed["scope"], "signed":signed,
        });
        let signed = canonical(&serde_json::to_vec(&wrapper).unwrap()).unwrap();
        // This is only a test of frozen signing-input bytes, not a verifier.
        let mut input = b"pinvou-upgrade-signed-v1\0".to_vec();
        input.extend(signed);
        assert!(
            hex(&input) == vector["expectedSigningInputHex"].as_str().unwrap(),
            "frozen signing wrapper bytes mismatch"
        );
    }
}

#[test]
fn ecmascript_finite_binary64_and_utf16_bytes() {
    for (input, expected) in [
        ("9007199254740993", "9007199254740992"),
        ("18446744073709551615", "18446744073709552000"),
        ("-9223372036854775808", "-9223372036854776000"),
        ("-0", "0"),
        ("-0.0", "0"),
        ("-1e-999", "0"),
        (
            "[1e-6,1e-7,1e20,1e21,5e-324]",
            "[0.000001,1e-7,100000000000000000000,1e+21,5e-324]",
        ),
        ("1.7976931348623157e308", "1.7976931348623157e+308"),
        ("333333333.33333329", "333333333.3333333"),
        ("{\"\u{e000}\":1,\"😀\":2}", "{\"😀\":2,\"\u{e000}\":1}"),
        ("{\"é\":1,\"é\":2}", "{\"é\":2,\"é\":1}"),
        (r#""\ud83d\ude00""#, "\"😀\""),
        (
            r#"{"__proto__":{"safe":true},"constructor":1}"#,
            r#"{"__proto__":{"safe":true},"constructor":1}"#,
        ),
    ] {
        assert_eq!(canonical(input.as_bytes()).unwrap(), expected.as_bytes());
    }
    assert!(canonical(b"1e999").is_err());
    assert!(canonical(b"-1e999").is_err());
    let document = Document::parse(b"1", InputKind::Metadata).unwrap();
    assert!(document.value().as_number().unwrap().is_f64());
}

#[test]
fn duplicate_syntax_unicode_and_utf8_reject_without_payload_diagnostics() {
    for input in [
        r#"{"a":1,"a":2}"#,
        r#"{"a":{"x":1,"\u0078":2}}"#,
        r#"[{"x":1,"x":2}]"#,
    ] {
        assert!(matches!(
            canonical(input.as_bytes()),
            Err(Error::DuplicateKey)
        ));
    }
    for input in [
        "[1,]",
        "{\"a\":}",
        "{a:1}",
        "01",
        "true false",
        "1e",
        r#""\x20""#,
        "\u{feff}{}",
        "\u{b}{}",
        r#""\ud800""#,
        r#""\udc00""#,
        r#"{"\ud800":1}"#,
    ] {
        assert!(canonical(input.as_bytes()).is_err());
    }
    assert!(matches!(canonical(&[0xc0, 0xaf]), Err(Error::InvalidUtf8)));
    assert_eq!(Error::DuplicateKey.to_string(), "JSON_DUPLICATE_KEY");
    assert_eq!(Error::InvalidJson.to_string(), "JSON_INVALID");
}

#[test]
fn depth_and_per_container_members_exact_boundaries() {
    let nested =
        |depth: usize, inner: &str| format!("{}{}{}", "[".repeat(depth), inner, "]".repeat(depth));
    assert!(canonical(nested(32, "0").as_bytes()).is_ok());
    assert!(matches!(
        canonical(nested(33, "0").as_bytes()),
        Err(Error::DepthLimit)
    ));
    assert!(canonical(nested(33, "").as_bytes()).is_ok()); // root=0, deepest empty container=32.
    assert!(matches!(
        canonical(nested(34, "").as_bytes()),
        Err(Error::DepthLimit)
    ));
    let array = |count: usize| format!("[{}]", vec!["0"; count].join(","));
    assert!(canonical(array(4096).as_bytes()).is_ok());
    assert!(matches!(
        canonical(array(4097).as_bytes()),
        Err(Error::MemberLimit)
    ));
    assert!(canonical(format!("[{},{}]", array(4096), array(4096)).as_bytes()).is_ok());
    let object = |count: usize| {
        format!(
            "{{{}}}",
            (0..count)
                .map(|index| format!("\"k{index}\":0"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    assert!(canonical(object(4096).as_bytes()).is_ok());
    assert!(matches!(
        canonical(object(4097).as_bytes()),
        Err(Error::MemberLimit)
    ));
}

#[test]
fn decoded_string_and_key_utf8_limits_include_escapes() {
    for value in [
        "a".repeat(65536),
        "é".repeat(32768),
        r"\u00e9".repeat(32768),
    ] {
        let input = format!("\"{value}\"");
        assert!(canonical(input.as_bytes()).is_ok());
        assert!(matches!(
            canonical(format!("\"{value}a\"").as_bytes()),
            Err(Error::StringLimit)
        ));
        assert!(canonical(format!("{{\"{value}\":0}}").as_bytes()).is_ok());
        assert!(matches!(
            canonical(format!("{{\"{value}a\":0}}").as_bytes()),
            Err(Error::StringLimit)
        ));
    }
}

#[test]
fn fixed_wire_profiles_and_canonical_expansion_exact_limits() {
    for (kind, maximum) in [
        (InputKind::Metadata, 1048576),
        (InputKind::Credential, 65536),
    ] {
        let mut wire = vec![b' '; maximum];
        wire[0] = b'0';
        assert!(Document::parse(&wire, kind).is_ok());
        wire.push(b' ');
        assert!(matches!(
            Document::parse(&wire, kind),
            Err(Error::InputLimit)
        ));
    }
    // 47000 short 1e20 lexemes expand to 21-digit ECMAScript numbers. Twelve
    // bounded groups plus object/string delimiters cost 22*N+2*G+14 bytes.
    let groups = (0..47000)
        .collect::<Vec<_>>()
        .chunks(4096)
        .map(|group| format!("[{}]", vec!["1e20"; group.len()].join(",")))
        .collect::<Vec<_>>();
    let filler = 1048576 - (22 * 47000 + 2 * groups.len() + 14);
    let input = |extra: usize| {
        format!(
            "{{\"a\":[{}],\"z\":\"{}\"}}",
            groups.join(","),
            "a".repeat(filler + extra)
        )
    };
    assert_eq!(canonical(input(0).as_bytes()).unwrap().len(), 1048576);
    assert!(matches!(
        canonical(input(1).as_bytes()),
        Err(Error::OutputLimit)
    ));
}

#[test]
fn original_node_oracle_for_number_unicode_and_rejection_paths() {
    use base64::engine::general_purpose::STANDARD;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let Ok(reference) = std::env::var("PINVOU_UPGRADE_NODE_REFERENCE") else {
        eprintln!("original Node oracle not configured; no oracle verification claimed");
        return;
    };
    let mut samples = vec![
        b"9007199254740993".to_vec(),
        b"18446744073709551615".to_vec(),
        b"-0".to_vec(),
        b"[1e-6,1e-7,1e20,1e21,5e-324,1.7976931348623157e308]".to_vec(),
        b"1e-9999".to_vec(),
        b"1e9999".to_vec(),
        b"1e000000000000000000000000000000000000000001".to_vec(),
        "{\"\u{e000}\":1,\"😀\":2,\"é\":3,\"é\":4}"
            .as_bytes()
            .to_vec(),
        br#"{"a":{"x":1,"\u0078":2}}"#.to_vec(),
        br#""\ud800""#.to_vec(),
        vec![0xc0, 0xaf],
        b"true false".to_vec(),
        "\u{feff}{}".as_bytes().to_vec(),
    ];
    for depth in [32, 33, 34] {
        for inner in ["", "0"] {
            samples
                .push(format!("{}{}{}", "[".repeat(depth), inner, "]".repeat(depth)).into_bytes());
        }
    }
    let encoded = samples
        .iter()
        .map(|sample| STANDARD.encode(sample))
        .collect::<Vec<_>>();
    let mut child = Command::new("node")
        .args([
            "--input-type=module",
            "--eval",
            include_str!("node-oracle.mjs"),
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
        .write_all(&serde_json::to_vec(&encoded).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "original oracle process unavailable"
    );
    let results: Vec<Option<String>> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), samples.len());
    for (sample, result) in samples.iter().zip(results) {
        match (canonical(sample), result) {
            (Ok(actual), Some(expected)) => assert_eq!(actual, STANDARD.decode(expected).unwrap()),
            (Err(_), None) => {}
            _ => panic!("original Node acceptance mismatch"),
        }
    }
}
