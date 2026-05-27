# Releasing FBX2U3D

## Requirements

- Rust toolchain: `stable-x86_64-pc-windows-gnu`
- MinGW in `C:\msys64\mingw64\bin`
- Inno Setup 6 in `C:\Users\ajones\AppData\Local\Programs\Inno Setup 6`
- Vendored Intel U3D SDK archive in `third_party\u3d-sdk\U3D_A_061228_5.zip` or an installed Intel U3D runtime under `%LOCALAPPDATA%\fbx2u3d\u3d-sdk\U3D_A_061228_5`

## Build the Installer

Run:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-installer.ps1
```

That script will:

1. build `target\release\fbx2u3d.exe`
2. stage the executable, docs, and bundled Intel runtime under `dist\staging\FBX2U3D`
3. compile `installer\FBX2U3D.iss` with Inno Setup
4. emit the installer into `dist\`

The packaging script prefers the vendored SDK archive first so release builds remain possible even if the upstream download URL disappears.

## Installer Features

- bundles the Intel U3D runtime required by `IDTFConverter.exe`
- optionally adds the install directory to the per-user Windows `PATH`
- optionally adds a right-click quick-convert entry for `.fbx` files
- installs documentation and license files beside the executable

## Git Prep

Before the first commit, review the generated files in `dist\` and exclude them from version control. `.gitignore` already covers the release output and generated `.u3d` artifacts.