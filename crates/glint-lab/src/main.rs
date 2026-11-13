// SPDX-License-Identifier: MIT
//! `glint-lab`: the Glint test lab (PLAN §16). Each run starts its own Xvfb and its own `glintd`
//! (found next to this program, or on `$PATH`), so it never clicks on the real desktop, unless
//! `--real-desktop` asks for exactly that.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use glint_core::profile::{Backend, Speed};
use glint_lab::{LATER, Options, Report, SCENARIOS};

#[derive(Parser, Debug)]
#[command(
    name = "glint-lab",
    version,
    about = "Glint's test lab: scenarios in a sandboxed X server (MIT)"
)]
struct Cli {
    /// The scenario, or `all` to run every one in the sandbox; `--list` shows them.
    #[arg(required_unless_present = "list")]
    scenario: Option<String>,
    /// List the scenarios.
    #[arg(long)]
    list: bool,
    /// Appearances, flashes or rounds (default: the scenario's own).
    #[arg(long)]
    count: Option<u32>,
    /// Seed for the random timings.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// eco, balanced, max, or checks per second.
    #[arg(long, default_value = "balanced", value_parser = glint_lab::parse_speed)]
    speed: Speed,
    /// auto, screen, screen-shm (shared memory), or window-buffer (window-mode scenarios read the
    /// window's own image).
    #[arg(long, default_value = "auto", value_parser = glint_lab::parse_backend)]
    backend: Backend,
    /// How long `soak` runs.
    #[arg(long, default_value_t = 60.0)]
    minutes: f64,
    /// The engine to test (default: `glintd` next to this program, then `$PATH`).
    #[arg(long)]
    glintd: Option<PathBuf>,
    /// Run on the real desktop ($DISPLAY) instead of a sandbox: a full-monitor test window takes
    /// the clicks. F8 stops the engine at any time.
    #[arg(long)]
    real_desktop: bool,
}

fn list() {
    println!("Scenarios (PLAN §16):");
    for s in SCENARIOS {
        let only = s.sandbox_only.map_or(String::new(), |_| " [sandbox only]".into());
        println!("  {:<21} {}{only}", s.name, s.about);
    }
    for (name, why) in LATER {
        println!("  {name:<21} not yet: {why}");
    }
}

fn verdict(report: &Report) -> bool {
    print!("{}", report.summary());
    report.passed()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.list {
        list();
        return ExitCode::SUCCESS;
    }
    let Some(scenario) = cli.scenario.clone() else {
        return ExitCode::FAILURE;
    };
    let glintd = cli.glintd.clone().unwrap_or_else(|| {
        let next_to_us = std::env::current_exe().ok().map(|p| p.with_file_name("glintd"));
        next_to_us
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("glintd"))
    });
    let opts = Options {
        count: cli.count,
        seed: cli.seed,
        speed: cli.speed,
        backend: cli.backend,
        duration: Duration::from_secs_f64(cli.minutes.max(0.0) * 60.0),
    };
    if cli.real_desktop {
        if scenario == "all" {
            eprintln!("glint-lab: `all` runs only in the sandbox");
            return ExitCode::FAILURE;
        }
        match glint_lab::scenario(&scenario) {
            Ok(s) if s.sandbox_only.is_none() => {}
            Ok(s) => {
                eprintln!(
                    "glint-lab: {} runs only in the sandbox: {}",
                    s.name,
                    s.sandbox_only.unwrap_or_default()
                );
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("glint-lab: {e}");
                return ExitCode::FAILURE;
            }
        }
        eprintln!("glint-lab: clicking inside a full-screen test window on the real desktop; F8 stops it.");
        for n in (1..=3).rev() {
            eprintln!("  starting in {n}…");
            std::thread::sleep(Duration::from_secs(1));
        }
        return match glint_lab::run_on_real_desktop(&glintd, &scenario, &opts) {
            Ok(report) if verdict(&report) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("glint-lab: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let names: Vec<&str> = if scenario == "all" {
        SCENARIOS.iter().map(|s| s.name).collect()
    } else {
        vec![scenario.as_str()]
    };
    let mut failed = Vec::new();
    for name in names {
        match glint_lab::run_in_sandbox(&glintd, name, &opts) {
            Ok(Some(report)) => {
                if !verdict(&report) {
                    failed.push(name);
                }
            }
            Ok(None) => {
                eprintln!("glint-lab: Xvfb is not installed");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("glint-lab {name}: {e}");
                failed.push(name);
            }
        }
        println!();
    }
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!("glint-lab: failed: {}", failed.join(", "));
        ExitCode::FAILURE
    }
}
