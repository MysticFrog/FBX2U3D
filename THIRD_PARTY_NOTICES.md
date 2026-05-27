# Third-Party Notices

This project redistributes the Intel Universal 3D Sample Software runtime files required to run `IDTFConverter.exe` on Windows.

## Intel Universal 3D Sample Software

- Components bundled by the installer: `IDTFConverter.exe`, `IFXCore.dll`, and the `Plugins` runtime DLLs from `U3D_A_061228_5\Bin\Win32\Release`
- Offline mirror bundled in the repository: `third_party\u3d-sdk\U3D_A_061228_5.zip`
- Upstream license: Apache License 2.0
- Upstream project: https://sourceforge.net/projects/u3d/

The full Apache 2.0 text for the Intel SDK is included in this repository as `third_party\u3d-sdk\LICENSE-APACHE-2.0.txt` and is installed alongside the bundled runtime.

## Rust Crates

Build-time and runtime Rust crate dependencies are resolved through Cargo and are not vendored into this repository. Their license metadata remains available through the Cargo ecosystem for release auditing.