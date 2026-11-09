// Coloca o ícone dentro do executável (Windows, toolchain GNU com windres).
use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=recursos/app.rc");
    println!("cargo:rerun-if-changed=recursos/icone.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let saida = PathBuf::from(env::var("OUT_DIR").unwrap()).join("icone.o");
    let ok = Command::new("windres")
        .args(["--input", "app.rc", "--output-format=coff", "--output"])
        .arg(&saida)
        .current_dir("recursos")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-link-arg-bins={}", saida.display());
    } else {
        println!("cargo:warning=windres não encontrado: o executável ficará sem ícone");
    }
}
