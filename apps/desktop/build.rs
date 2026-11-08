fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=lagom.rc");
        println!("cargo:rerun-if-changed=../../brand/app-icon/export/lagom.ico");
        embed_resource::compile("lagom.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embed the Windows app icon");
    }
}
