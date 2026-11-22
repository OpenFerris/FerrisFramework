//! `ferris shell`: a REPL over the `ferris` commands themselves.
//!
//! It used to run `cargo run -- <args>` in the app project. That cannot work:
//! `ferris new` writes a crate with `crate-type = ["cdylib"]` and a
//! `.cargo/config.toml` pinning `target = "wasm32-wasip2"`, so there is no binary
//! to run and `cargo run` fails at the manifest — after building the whole
//! workspace. Two smaller things went with it: the interactive loop did not
//! notice EOF (`read_line` returns `Ok(0)` there, not a line), so
//! `ferris shell < /dev/null` prompted forever, and `mod.rs` advertised an
//! interactive shell that had nothing to interact with.
//!
//! What a shell for this CLI can usefully be is the CLI: run the subcommands
//! without leaving the terminal, keeping the exit code of each one visible.

use crate::config::CliConfig;
use crate::project;
use anyhow::Result;
use std::io::{BufRead, Write};
use tracing::info;

/// What a line typed at the prompt means.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Nothing to do: a blank line.
    Empty,
    /// `exit`, `quit`, or Ctrl-D.
    Quit,
    /// `!<command>`: a shell command.
    Shell(String),
    /// A `ferris` subcommand and its arguments.
    Ferris(Vec<String>),
}

fn classify(line: &str) -> Action {
    let line = line.trim();
    if line.is_empty() {
        return Action::Empty;
    }
    if matches!(line, "exit" | "quit") {
        return Action::Quit;
    }
    if let Some(command) = line.strip_prefix('!') {
        return Action::Shell(command.trim().to_string());
    }
    Action::Ferris(line.split_whitespace().map(str::to_string).collect())
}

/// Read one line, distinguishing EOF from a blank line.
///
/// `read_line` reports EOF as `Ok(0)` with an untouched buffer, which is an empty
/// string — the same thing a user pressing Enter produces. Treating both as
/// "empty line" and continuing is an infinite loop on a closed stdin.
fn read_line(input: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    match input.read_line(&mut line)? {
        0 => Ok(None),
        _ => Ok(Some(line)),
    }
}

pub fn run(args: super::ShellArgs, config: &CliConfig) -> Result<()> {
    project::check_project_exists(&config.source.project_dir)?;

    if args.command.is_empty() {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut prompt = std::io::stdout();
        run_repl(&mut input, &mut prompt, dispatch)
    } else {
        let code = dispatch(&args.command);
        if code == 0 {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "`ferris {}` exited with {code}",
                args.command.join(" ")
            ))
        }
    }
}

fn dispatch(command: &[String]) -> i32 {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("cannot find the ferris executable to re-run: {e}");
            return 127;
        }
    };
    std::process::Command::new(executable)
        .args(command)
        .status()
        .map_or_else(
            |e| {
                eprintln!("could not run ferris: {e}");
                127
            },
            |status| status.code().unwrap_or(1),
        )
}

/// The loop, over injectable streams so that end-of-input is testable.
fn run_repl(
    input: &mut impl BufRead,
    prompt: &mut impl Write,
    mut dispatch: impl FnMut(&[String]) -> i32,
) -> Result<()> {
    loop {
        write!(prompt, "ferris> ")?;
        prompt.flush()?;

        // `None` is end of input: leave instead of asking again.
        let Some(line) = read_line(input)? else {
            break;
        };

        match classify(&line) {
            // A blank line just asks again.
            Action::Empty => {}
            Action::Quit => break,
            Action::Shell(command) => {
                // A failing command inside the shell is not the shell's failure.
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .status();
            }
            Action::Ferris(command) => {
                let code = dispatch(&command);
                if code != 0 {
                    info!("`ferris {}` exited with {code}", command.join(" "));
                }
            }
        }
    }

    writeln!(prompt)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn end_of_input_is_not_a_blank_line() {
        // The bug: `Ok(0)` with an empty buffer read as "the user pressed Enter",
        // so `ferris shell < /dev/null` prompted forever.
        let mut closed = Cursor::new("");
        assert_eq!(
            read_line(&mut closed).unwrap(),
            None,
            "a closed stdin must read as end of input, not as an empty line"
        );

        let mut blank = Cursor::new("\n\n");
        assert_eq!(read_line(&mut blank).unwrap().as_deref(), Some("\n"));
        assert_eq!(read_line(&mut blank).unwrap().as_deref(), Some("\n"));
        assert_eq!(read_line(&mut blank).unwrap(), None, "then end of input");
    }

    #[test]
    fn the_repl_ends_on_closed_input() {
        let mut input = Cursor::new("");
        let mut output = Vec::new();
        let mut calls = 0;
        run_repl(&mut input, &mut output, |_| {
            calls += 1;
            0
        })
        .expect("a closed stdin is not an error");
        assert_eq!(calls, 0, "nothing to dispatch");
        assert!(!output.is_empty(), "the prompt was written before the read");
    }

    #[test]
    fn lines_are_routed_and_the_loop_exits() {
        let mut input = Cursor::new("  \ninfo\nexit\nnever-run\n");
        let mut output = Vec::new();
        let mut seen: Vec<Vec<String>> = Vec::new();
        run_repl(&mut input, &mut output, |command| {
            seen.push(command.to_vec());
            1
        })
        .unwrap();
        assert_eq!(seen, vec![vec!["info".to_string()]], "`exit` ends the loop");
    }

    #[test]
    fn what_each_line_means() {
        assert_eq!(classify("   "), Action::Empty);
        assert_eq!(classify("exit"), Action::Quit);
        assert_eq!(classify("quit"), Action::Quit);
        assert_eq!(classify("!ls -la"), Action::Shell("ls -la".into()));
        assert_eq!(
            classify("  config show  "),
            Action::Ferris(vec!["config".into(), "show".into()])
        );
    }
}
