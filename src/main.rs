mod cli_app;

use paradown::Error;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match cli_app::run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("paradown: {err}");
            map_error_to_exit_code(&err)
        }
    }
}

fn map_error_to_exit_code(err: &Error) -> ExitCode {
    match err {
        Error::ConfigError(_)
        | Error::InvalidUrl(_)
        | Error::UrlParseError(_)
        | Error::Parse(_)
        | Error::Header(_) => ExitCode::from(64),
        Error::UnsupportedProtocol(_) => ExitCode::from(65),
        Error::TaskNotFound(_) | Error::Canceled(_) => ExitCode::from(2),
        _ => ExitCode::from(1),
    }
}
