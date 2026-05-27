# fbx2u3d

Rust CLI workspace for converting CAD-oriented FBX assets into U3D packages.

This repository is prepared for publication at https://github.com/MysticFrog/FBX2U3D.

## Current scope

- Converts binary FBX 7.x mesh geometry into U3D through an IDTF intermediate.
- Carries FBX material slots, the first UV layer, and connected diffuse textures through the generated IDTF before running the Intel converter.
- Preserves FBX assembly/component/body structure by emitting separate IDTF group and model nodes so individual items remain identifiable in the exported U3D scene graph.
- Extracts polygon mesh data directly in Rust and uses Intel's Apache-licensed `IDTFConverter.exe` for the final U3D encoding step.
- Supports a bundled Windows installer layout by auto-discovering `IDTFConverter.exe` relative to the installed `fbx2u3d.exe` when the Intel runtime is shipped with the app.
- Includes VS Code tasks that call Cargo from `%USERPROFILE%\\.cargo\\bin`, which is present on this machine even though it is not on the terminal `PATH`.
- Pins the workspace to the installed `stable-x86_64-pc-windows-gnu` toolchain and prepends `C:\msys64\mingw64\bin` so GNU linker tools like `dlltool.exe` are available during builds.
- Redirects Cargo build artifacts to `C:/Users/ajones/.cargo-targets/fbx2u3d` because MinGW tooling fails when the default target directory lives under `J:\My Drive\...` with spaces in the path.

## Commands

Install the U3D converter locally under `%LOCALAPPDATA%\fbx2u3d\u3d-sdk`:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-u3d-sdk.ps1
```

If the upstream SourceForge URL is unavailable, `install-u3d-sdk.ps1` now prefers the vendored archive in `third_party\u3d-sdk\U3D_A_061228_5.zip`.

Run a dry plan against an existing FBX file:

```powershell
& "$HOME\.cargo\bin\cargo.exe" run -- path\to\model.fbx --dry-run
```

Dry-run output now includes scene node and mesh part counts, which is useful for confirming that an assembly is not being flattened into a single U3D object.

Run the test suite:

```powershell
& "$HOME\.cargo\bin\cargo.exe" test
```

Inside the VS Code workspace terminal, `.vscode/settings.json` adds both Cargo and `C:\msys64\mingw64\bin` to `PATH`, so plain `cargo check` and `cargo test` will work there as well.

Build a signed-ready Windows installer with the bundled Intel runtime:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-installer.ps1
```

The installer is authored with Inno Setup 6 and currently supports optional per-user PATH registration and an Explorer right-click quick-convert entry for `.fbx` files.

## Backend notes

- The current importer supports binary FBX 7.x files. ASCII FBX is not supported yet because the Rust parser backend is binary-only.
- The CLI auto-discovers `IDTFConverter.exe` in `%LOCALAPPDATA%\fbx2u3d\u3d-sdk\U3D_A_061228_5\Bin\Win32\Release\IDTFConverter.exe` or uses `--idtf-converter <PATH>`.
- When distributed through the installer, the CLI also auto-discovers the bundled converter under `u3d-sdk\U3D_A_061228_5\Bin\Win32\Release` next to `fbx2u3d.exe`.
- Mesh export now writes polygon geometry, FBX source normals when `LayerElementNormal` data is present (falling back to generated flat normals otherwise), per-face material assignments, one diffuse texture layer when the FBX contains `LayerElementMaterial`, `LayerElementUV`, and material-texture connections, and separate scene nodes for preserved FBX hierarchy.
- The CLI drives `IDTFConverter.exe` with maximum position, texture-coordinate, normal, and geometry quality settings to reduce Acrobat-visible quantization on dense CAD meshes.
- Current gaps: layered textures beyond the first diffuse map, animation, skinning, lights, and cameras are not exported yet.

## Licensing

- Commercial use of the original FBX2U3D project files is not permitted without prior written agreement. That rule is intentionally emphasized in `LICENSE`.
- The bundled Intel U3D SDK remains Apache-2.0 licensed and keeps its own Apache-granted downstream rights. Those third-party license terms are documented separately in `THIRD_PARTY_NOTICES.md` and `third_party\u3d-sdk\LICENSE-APACHE-2.0.txt`.
- Mixed-license downstream redistributions should preserve those boundaries instead of merging everything under a single blanket statement.

## Example

```powershell
& "$HOME\.cargo\bin\cargo.exe" run -- path\to\assembly.fbx --output path\to\assembly.u3d
```

If you keep the converter elsewhere, pass it explicitly:

```powershell
& "$HOME\.cargo\bin\cargo.exe" run -- path\to\assembly.fbx --idtf-converter C:\path\to\IDTFConverter.exe
```

See `RELEASING.md` for packaging and installer build steps.