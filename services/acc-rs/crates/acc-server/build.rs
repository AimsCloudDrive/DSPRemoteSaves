use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_web_dist)");

    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist/index.html");
    if dist.exists() {
        println!("cargo:rustc-cfg=has_web_dist");
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
