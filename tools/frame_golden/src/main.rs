//! The frame_golden binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match frame_golden::cli::parse(&args).and_then(frame_golden::cli::execute) {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("frame_golden: {err}");
            ExitCode::FAILURE
        }
    }
}
