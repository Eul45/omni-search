fn main() {
    println!("cargo:rerun-if-changed=ui/settings.slint");
    println!("cargo:rerun-if-changed=icons/OmniSearchTrans.ico");
    println!("cargo:rerun-if-changed=icons/OmniSearchTrans.png");
    println!("cargo:rerun-if-changed=icons/OmniSearchTrans_small.png");
    println!("cargo:rerun-if-changed=icons/OmniSearchTrans_16.png");
    println!("cargo:rerun-if-changed=icons/OmniSearchTrans_32.png");

    slint_build::compile("ui/settings.slint").unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("icons/OmniSearchTrans.ico");
        res.set_language(0x0409); // U.S. English
        res.set("FileDescription", "OmniSearch Lite Launcher");
        res.set("ProductName", "OmniSearch Lite");
        res.set("OriginalFilename", "omnisearch-lite.exe");
        res.set("CompanyName", "Eyuel Engida");
        res.set("LegalCopyright", "Copyright (c) 2026 Eyuel Engida");
        res.set("FileVersion", "1.0.0.0");
        res.set("ProductVersion", "1.0.0.0");
        res.compile().unwrap();
    }
}
