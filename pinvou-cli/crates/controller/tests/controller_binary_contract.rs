use std::process::Command;

#[test]
fn binary_can_validate_local_configuration_without_starting_a_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_pinvou-controller"))
        .arg("--check-config")
        .output()
        .expect("controller binary must run");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "controller configuration ok"
    );
    assert!(output.stderr.is_empty());
}
