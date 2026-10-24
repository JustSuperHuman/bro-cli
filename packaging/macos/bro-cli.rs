//! Native terminal launcher inside Bro.app, signed before the enclosing bundle.
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("bro-cli: could not find its executable: {error}");
            std::process::exit(1);
        }
    };
    let error = Command::new(executable.with_file_name("bro"))
        .arg("--tui")
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("bro-cli: could not start bro: {error}");
    std::process::exit(1);
}
