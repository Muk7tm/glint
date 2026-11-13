// SPDX-License-Identifier: GPL-3.0-or-later
//! `glint-lab`: the Glint test lab (PLAN §16). Each run starts its own Xvfb and its own `glintd`
//! (found next to this program, or on `$PATH`), so it never clicks on the real desktop, unless
//! `--real-desktop` asks for exactly that.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "glint-lab",
    version,
    about = "Glint's test lab: scenarios in a sandboxed X server (GPL-3.0-or-later)"
)]
struct Cli {
    /// basic, near-miss or arm-while-present
    scenario: String,
    /// How many appearances (or near misses) to show.
    #[arg(long, default_value_t = 100)]
    count: u32,
    /// Seed for the random timings.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// The engine to test (default: `glintd` next to this program, then `$PATH`).
    #[arg(long)]
    glintd: Option<PathBuf>,
    /// Run on the real desktop ($DISPLAY) instead of a sandbox: a full-monitor test window takes
    /// the clicks. F8 stops the engine at any time.
    #[arg(long)]
    real_desktop: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let glintd = cli.glintd.clone().unwrap_or_else(|| {
        let next_to_us = std::env::current_exe().ok().map(|p| p.with_file_name("glintd"));
        next_to_us
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("glintd"))
    });
    if cli.real_desktop {
        eprintln!("glint-lab: clicking inside a full-screen test window on the real desktop; F8 stops it.");
        for n in (1..=3).rev() {
            eprintln!("  starting in {n}…");
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        return match glint_lab::run_on_real_desktop(&glintd, &cli.scenario, cli.count, cli.seed) {
            Ok(report) => {
                print!("{}", report.summary());
                if report.passed() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(e) => {
                eprintln!("glint-lab: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match glint_lab::run_in_sandbox(&glintd, &cli.scenario, cli.count, cli.seed) {
        Ok(Some(report)) => {
            print!("{}", report.summary());
            if report.passed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Ok(None) => {
            eprintln!("glint-lab: Xvfb is not installed");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("glint-lab: {e}");
            ExitCode::FAILURE
        }
    }
}
