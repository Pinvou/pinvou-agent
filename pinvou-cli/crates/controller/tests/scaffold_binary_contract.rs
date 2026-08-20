use std::process::Command;

#[test]
fn scaffold_binary_reports_unavailable_as_a_general_failure() {
    let output = Command::new(env!("CARGO_BIN_EXE_pinvou-controller"))
        .output()
        .expect("controller scaffold binary must run");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "pinvou-controller is not implemented in the workspace scaffold"
    );
}
