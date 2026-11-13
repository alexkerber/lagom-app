fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=lagom.rc");
        println!("cargo:rerun-if-changed=../../brand/app-icon/export/lagom.ico");
        // docs/release.md: the tag is the only version source; development builds are 0.0.0.
        println!("cargo:rerun-if-env-changed=LAGOM_VERSION");
        let version = std::env::var("LAGOM_VERSION").unwrap_or_default();
        let mut parts = version.split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse::<u16>().unwrap_or(0));
        let defines = ["MAJOR", "MINOR", "PATCH"].map(|name| format!("LAGOM_{name}={}", parts.next().unwrap_or(0)));
        embed_resource::compile("lagom.rc", defines)
            .manifest_required()
            .expect("embed the Windows app icon");
    }
}
