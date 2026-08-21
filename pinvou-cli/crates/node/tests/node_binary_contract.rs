use std::process::Command;

#[test]
fn binary_validates_configuration_without_opening_network_listeners() {
    let output = Command::new(env!("CARGO_BIN_EXE_pinvou-node"))
        .arg("--check-config")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "node configuration ok"
    );
    assert!(output.stderr.is_empty());
}
