fn main() {
    println!("cargo:rerun-if-changed=cpp/scanner.cpp");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .cpp(true)
            .file("cpp/scanner.cpp")
            .flag_if_supported("/std:c++20")
            .flag_if_supported("/EHsc")
            .warnings(true)
            .compile("scanner");

        let windows = tauri_build::WindowsAttributes::new()
            .app_manifest(include_str!("windows-app-manifest.xml"));
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("failed to run tauri build script");
        return;
    }

    tauri_build::build();
}
