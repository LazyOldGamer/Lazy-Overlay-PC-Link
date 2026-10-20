// Windows: the icon, version details and manifest. A program that says who
// made it and asks for no admin rights looks less like malware.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/pclink.manifest");
    // The DLLs it imports from the system's folder only, never from beside it
    // (a Downloads folder holding a planted one): os.rs, harden, for the rest.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Lazy Overlay PC Link")
            .set("FileDescription", "Lazy Overlay PC Link")
            .set("CompanyName", "LazyOldGamer")
            .set("LegalCopyright", "LazyOldGamer")
            .set("OriginalFilename", "lazy-overlay-link.exe")
            .set("InternalName", "lazy-overlay-link")
            .set_manifest_file("assets/pclink.manifest");
        res.compile().expect("Windows resources");
    }
}
