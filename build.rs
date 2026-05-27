fn main() {
    println!("cargo:rerun-if-changed=FBX2U3D-MAC.ico");

    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("FBX2U3D-MAC.ico");
        resource
            .compile()
            .expect("failed to embed FBX2U3D-MAC.ico into the Windows executable");
    }
}