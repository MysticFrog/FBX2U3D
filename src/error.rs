use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("input file was not found: {0}")]
    InputMissing(PathBuf),

    #[error("input file must use the .fbx extension: {0}")]
    UnsupportedInput(PathBuf),

    #[error("output file must use the .u3d extension: {0}")]
    UnsupportedOutput(PathBuf),

    #[error("output directory does not exist: {0}")]
    OutputDirectoryMissing(PathBuf),

    #[error("output file already exists, pass --overwrite to replace it: {0}")]
    OutputExists(PathBuf),

    #[error("units scale must be greater than zero, got {0}")]
    InvalidUnitsScale(f32),

    #[error("IDTFConverter.exe was not found. Checked: {0}")]
    IdtfConverterMissing(String),

    #[error("the FBX input flavor is not supported by the current backend: {0}")]
    UnsupportedFbxFlavor(String),

    #[error("failed to parse FBX data: {0}")]
    FbxParse(String),

    #[error("the FBX file does not contain any polygon mesh geometry that this backend can export")]
    MeshMissing,

    #[error("the FBX file uses a feature this backend does not handle yet: {0}")]
    UnsupportedFbxFeature(String),

    #[error("U3D conversion failed: {0}")]
    ConversionFailed(String),

    #[error("I/O error while preparing conversion: {0}")]
    Io(#[from] std::io::Error),
}