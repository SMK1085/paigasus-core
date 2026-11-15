// SPDX-License-Identifier: Apache-2.0
//! `paigasus-policy-compiler compile` and `paigasus-policy-compiler revision` (spec 2026-11-15
//! iam-tool-policy-sets, sections 5.2 and 6.2).
//!
//! `compile` reads the whole stdin as one request and writes one JSON line to stdout. It exits 0
//! for an accepted and for a rejected set. A request that does not parse, or that has another
//! protocol version, gives one line on stderr and exit 2. Any other argument list exits 64.

use std::io::{Read, Write};
use std::process::ExitCode;

use paigasus_policy_compiler::protocol::{RevisionOutput, compile_request, parse_request};
use serde::Serialize;

const EXIT_BAD_REQUEST: u8 = 2;
const EXIT_USAGE: u8 = 64;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["compile"] => compile(),
        ["revision"] => write_line(&RevisionOutput::current()),
        _ => {
            eprintln!("usage: paigasus-policy-compiler compile|revision");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn compile() -> ExitCode {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        return bad_request(&format!("stdin is not readable UTF-8: {error}"));
    }
    match parse_request(&input).and_then(|request| compile_request(&request)) {
        Ok(result) => write_line(&result),
        Err(error) => bad_request(&error.to_string()),
    }
}

/// One line on stderr. A newline in the reason is escaped, so the line stays one line.
fn bad_request(reason: &str) -> ExitCode {
    eprintln!("{}", reason.replace('\n', "\\n"));
    ExitCode::from(EXIT_BAD_REQUEST)
}

/// Write `value` as one JSON line to stdout.
fn write_line(value: &impl Serialize) -> ExitCode {
    let line = match serde_json::to_string(value) {
        Ok(line) => line,
        Err(error) => {
            eprintln!("the result does not serialize: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{line}").and_then(|()| stdout.flush()).is_err() {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
