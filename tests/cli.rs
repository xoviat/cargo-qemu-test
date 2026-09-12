use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cargo-qtest")
}

#[test]
fn exposes_defmt_flags() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--defmt"), "help output:\n{help}");
    assert!(help.contains("--no-defmt"), "help output:\n{help}");
}

#[test]
fn defmt_and_no_defmt_conflict() {
    let out = Command::new(bin())
        .args(["--defmt", "--no-defmt", "--timeout", "1"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap must reject conflicting flags: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
