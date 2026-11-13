fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=lagom.rc");
        println!("cargo:rerun-if-changed=../../brand/app-icon/export/lagom.ico");
        let version = |part: &str| format!("LAGOM_{part}={}", std::env::var(format!("CARGO_PKG_VERSION_{part}")).unwrap());
        embed_resource::compile("lagom.rc", ["MAJOR", "MINOR", "PATCH"].map(version))
            .manifest_required()
            .expect("embed the Windows app icon");
    }
}
