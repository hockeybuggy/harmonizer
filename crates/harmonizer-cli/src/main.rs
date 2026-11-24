use clap::Parser;
use harmonizer_cli::{Cli, run};

fn main() {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(summary) => eprintln!("{}", summary.format_line()),
        Err(err) => {
            eprintln!("harmonizer: {err:#}");
            std::process::exit(1);
        }
    }
}
