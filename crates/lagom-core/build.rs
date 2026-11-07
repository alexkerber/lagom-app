fn main() {
    println!("cargo:rustc-check-cfg=cfg(lagom_greenmail)");
    println!("cargo:rerun-if-env-changed=LAGOM_GREENMAIL");
    if std::env::var("LAGOM_GREENMAIL").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=lagom_greenmail");
    }
}
