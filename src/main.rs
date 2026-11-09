use mujo::diagnose;
use mujo::event_loop;

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    style::Print,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;

/// Actions that must be resolved before any terminal setup.
#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Diagnose,
    Version,
    Help,
    Run,
}

/// Classify the process arguments into the action to take. Mirrors the
/// shape --diagnose used so --version/--help also resolve before any
/// terminal setup (no alternate screen, no raw mode). Anything
/// unrecognised falls through to starting the TUI.
fn classify_args<I, S>(args: I) -> CliAction
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut it = args.into_iter();
    let _program = it.next();
    for arg in it {
        match arg.as_ref() {
            "--diagnose" => return CliAction::Diagnose,
            "--version" | "-V" => return CliAction::Version,
            "--help" | "-h" => return CliAction::Help,
            _ => {}
        }
    }
    CliAction::Run
}

fn main() -> anyhow::Result<()> {
    match classify_args(std::env::args()) {
        CliAction::Diagnose => {
            println!("{}", diagnose::report());
            return Ok(());
        }
        CliAction::Version => {
            println!("mujo {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        CliAction::Help => {
            println!("mujo - a TUI front end for pass");
            println!();
            println!("Usage: mujo [--diagnose | --version | --help]");
            println!("  --diagnose  print an environment report and exit");
            println!("  --version   print the version and exit");
            println!("  --help      print this help and exit");
            println!();
            println!("Key bindings are documented inside the TUI via '?'.");
            return Ok(());
        }
        CliAction::Run => {}
    }
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, Print("\x1b7"))?;
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let res = event_loop::run_app();
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen, DisableMouseCapture)?;
    execute!(stdout, Print("\x1b8"))?;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_args_version_flags() {
        assert_eq!(classify_args(["mujo", "--version"]), CliAction::Version);
        assert_eq!(classify_args(["mujo", "-V"]), CliAction::Version);
    }

    #[test]
    fn classify_args_help_flags() {
        assert_eq!(classify_args(["mujo", "--help"]), CliAction::Help);
        assert_eq!(classify_args(["mujo", "-h"]), CliAction::Help);
    }

    #[test]
    fn classify_args_diagnose_flag() {
        assert_eq!(classify_args(["mujo", "--diagnose"]), CliAction::Diagnose);
    }

    #[test]
    fn classify_args_default_is_run() {
        assert_eq!(classify_args(["mujo"]), CliAction::Run);
        assert_eq!(classify_args(Vec::<String>::new()), CliAction::Run);
        assert_eq!(classify_args(["mujo", "--glyphs", "ascii"]), CliAction::Run);
    }
}
