use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for (shell, flag) in [("bash", "has_bash"), ("fish", "has_fish")] {
        println!("cargo::rustc-check-cfg=cfg({flag})");
        let found = Command::new(shell).arg("--version").output().is_ok_and(|output| output.status.success());
        if found {
            println!("cargo::rustc-cfg={flag}");
        }
    }
}
