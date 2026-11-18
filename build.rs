use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-env-changed=PATH");
    println!("cargo::rerun-if-env-changed=RC_SORT_REQUIRE_SHELLS");
    for (shell, flag) in [("bash", "has_bash"), ("fish", "has_fish")] {
        println!("cargo::rustc-check-cfg=cfg({flag})");
        if Command::new(shell).arg("--version").output().is_ok() {
            println!("cargo::rustc-cfg={flag}");
        } else if std::env::var_os("RC_SORT_REQUIRE_SHELLS").is_some() {
            panic!("{shell} is required but was not found on PATH");
        }
    }
}
