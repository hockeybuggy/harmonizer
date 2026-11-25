use std::process::ExitCode;

#[cfg(target_os = "macos")]
mod smoke;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    match smoke::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vst3-smoke: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("vst3-smoke requires macOS");
    ExitCode::FAILURE
}
