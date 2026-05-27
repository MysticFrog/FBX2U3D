use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum, ValueHint};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "fbx2u3d",
    version,
    about = "Plan and run FBX to U3D conversions for CAD assets."
)]
pub struct Cli {
    #[arg(value_name = "INPUT", value_hint = ValueHint::FilePath)]
    pub input: PathBuf,

    #[arg(short, long, value_name = "OUTPUT", value_hint = ValueHint::AnyPath)]
    pub output: Option<PathBuf>,

    #[arg(long, action = ArgAction::SetTrue, help = "Replace an existing output file.")]
    pub overwrite: bool,

    #[arg(
        long,
        default_value_t = 1.0,
        help = "Scale applied to imported mesh units before export."
    )]
    pub units_scale: f32,

    #[arg(long, value_enum, default_value_t = Backend::Idtf, help = "Conversion backend to use.")]
    pub backend: Backend,

    #[arg(
        long,
        value_name = "PATH",
        value_hint = ValueHint::FilePath,
        help = "Path to IDTFConverter.exe. If omitted, the app checks U3D_IDTF_CONVERTER and the local SDK install path."
    )]
    pub idtf_converter: Option<PathBuf>,

    #[arg(
        long,
        action = ArgAction::SetTrue,
        help = "Validate inputs and print the resolved conversion plan without writing output."
    )]
    pub dry_run: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum Backend {
    Idtf,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idtf => "idtf",
        }
    }
}