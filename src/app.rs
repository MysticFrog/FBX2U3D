use crate::{cli::Cli, conversion, error::AppError};

pub fn run(cli: Cli) -> Result<String, AppError> {
    let plan = conversion::build_plan(&cli)?;
    conversion::execute(&plan)
}