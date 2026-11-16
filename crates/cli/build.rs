fn main() {
    #[cfg(windows)]
    {
        let icon =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/icons/icon.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        assert!(
            icon.is_file(),
            "Generate the approved desktop icons before building the Windows CLI"
        );
        let mut resource = tauri_winres::WindowsResource::new();
        resource
            .set_icon_with_id(icon.to_str().expect("CLI icon path must be UTF-8"), "1")
            .set("ProductName", "Numera Video Sub Finder")
            .set("FileDescription", "Numera Video Sub Finder CLI")
            .set("OriginalFilename", "nvsf.exe");
        resource
            .compile_for(&["nvsf"])
            .expect("Windows CLI resources must compile successfully");
    }
}
