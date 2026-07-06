//! runupdate — run the usual set of Debian/Ubuntu system-update commands, with
//! an optional capability-based mode that avoids needing `sudo` at update time.
//!
//! Modes:
//!   (default)   run the update commands and report their output
//!   setup       grant this binary the required file capabilities (needs root)
//!   teardown    remove those file capabilities again (needs root)

mod caps;

use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const PROG: &str = "runupdate";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The update commands run, in order, by the default mode.
/// Each entry is (program, args, required?). Non-required commands (snap) are
/// skipped with a note when the program is not installed.
const UPDATE_COMMANDS: &[(&str, &[&str], bool)] = &[
    ("snap", &["refresh"], false),
    ("apt", &["update"], true),
    ("apt", &["upgrade", "-y"], true),
    ("apt", &["autoremove", "-y"], true),
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    // A single mode/flag argument is expected. Anything else is a usage error.
    match args.first().map(String::as_str) {
        None => run_updates(),
        Some("-h") | Some("--help") | Some("help") => {
            print_help();
            ExitCode::SUCCESS
        }
        Some("-V") | Some("--version") | Some("version") => {
            println!("{PROG} {VERSION}");
            ExitCode::SUCCESS
        }
        Some("setup") | Some("--setup") => setup_caps(),
        Some("teardown") | Some("--teardown") | Some("remove") | Some("--remove") => teardown_caps(),
        Some(other) => {
            eprintln!("{PROG}: unknown argument: {other}\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// Default mode: elevate to root using the granted capabilities (if any) and
/// run each update command, streaming its output and reporting a summary.
fn run_updates() -> ExitCode {
    // apt/dpkg/snap require a real root UID, so use the granted CAP_SETUID/
    // CAP_SETGID to become root before running them.
    let elevated = caps::become_root();

    if !elevated {
        eprintln!(
            "{PROG}: note: not running as root and unable to become root. The \
             commands below will likely fail with permission errors.\n      Run \
             `sudo {PROG} setup` once to grant the required capabilities, or run \
             this tool with sudo.\n"
        );
    }

    let mut failures: Vec<String> = Vec::new();

    for (prog, cmd_args, required) in UPDATE_COMMANDS {
        let pretty = format!("{prog} {}", cmd_args.join(" "));
        println!("==> {pretty}");

        match Command::new(prog).args(*cmd_args).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                let code = status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                eprintln!("--- `{pretty}` exited with status {code}");
                failures.push(pretty);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if *required {
                    eprintln!("--- `{prog}` is not installed");
                    failures.push(pretty);
                } else {
                    println!("--- `{prog}` is not installed; skipping");
                }
            }
            Err(e) => {
                eprintln!("--- failed to run `{pretty}`: {e}");
                failures.push(pretty);
            }
        }
        println!();
    }

    if failures.is_empty() {
        println!("All update commands completed successfully.");
        ExitCode::SUCCESS
    } else {
        eprintln!("{} command(s) failed:", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        ExitCode::FAILURE
    }
}

/// `setup` mode: grant the required file capabilities to this binary via
/// `setcap`. Requires root (CAP_SETFCAP).
fn setup_caps() -> ExitCode {
    if !caps::is_root() {
        eprintln!("{PROG}: `setup` must be run as root (try `sudo {PROG} setup`).");
        return ExitCode::FAILURE;
    }

    let exe = match current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{PROG}: cannot determine path to this executable: {e}");
            return ExitCode::FAILURE;
        }
    };

    let spec = match caps::setcap_spec() {
        Some(s) => s,
        None => {
            eprintln!(
                "{PROG}: none of the required capabilities are available in this \
                 environment's bounding set; cannot grant any. Run the update \
                 commands under sudo instead."
            );
            return ExitCode::FAILURE;
        }
    };

    let skipped = caps::unavailable_caps();
    if !skipped.is_empty() {
        eprintln!(
            "{PROG}: warning: these capabilities are not in this environment's \
             bounding set and will be skipped:\n      {}\n      (granting them \
             would make this binary unexecutable here.)\n",
            skipped.join(", ")
        );
    }

    println!("Granting capabilities to {}:", exe.display());
    println!("  {spec}");

    match Command::new("setcap").arg(&spec).arg(&exe).status() {
        Ok(status) if status.success() => {
            println!("\nDone. You can now run `{PROG}` without sudo.");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("{PROG}: setcap failed with status {status}");
            ExitCode::FAILURE
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "{PROG}: `setcap` not found. Install it with `apt install libcap2-bin`."
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("{PROG}: failed to run setcap: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `teardown` mode: remove all file capabilities from this binary via
/// `setcap -r`. Requires root.
fn teardown_caps() -> ExitCode {
    if !caps::is_root() {
        eprintln!("{PROG}: `teardown` must be run as root (try `sudo {PROG} teardown`).");
        return ExitCode::FAILURE;
    }

    let exe = match current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{PROG}: cannot determine path to this executable: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("Removing capabilities from {}", exe.display());

    match Command::new("setcap").arg("-r").arg(&exe).status() {
        Ok(status) if status.success() => {
            println!("Done. Capabilities removed; `{PROG}` now needs sudo again.");
            ExitCode::SUCCESS
        }
        Ok(status) => {
            eprintln!("{PROG}: setcap -r failed with status {status}");
            ExitCode::FAILURE
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "{PROG}: `setcap` not found. Install it with `apt install libcap2-bin`."
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("{PROG}: failed to run setcap: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the real path to the running executable, following symlinks so that
/// `setcap` operates on the actual file.
fn current_exe() -> std::io::Result<PathBuf> {
    let exe = env::current_exe()?;
    // Canonicalize so capabilities are applied to the real file, not a symlink.
    exe.canonicalize().or(Ok(exe))
}

fn print_help() {
    println!(
        "\
{PROG} {VERSION}
Run the standard system-update commands, optionally without sudo by granting
this binary the required Linux capabilities.

USAGE:
    {PROG}              Run the update commands and report their output:
                          snap refresh
                          apt update
                          apt upgrade -y
                          apt autoremove -y

    {PROG} setup        Grant this binary the capabilities it needs to run the
                        update commands without sudo (see HOW IT WORKS).
                        Must be run as root (e.g. `sudo {PROG} setup`).

    {PROG} teardown     Remove those capabilities again. Must be run as root
                        (e.g. `sudo {PROG} teardown`).

    {PROG} --help       Show this help.
    {PROG} --version    Show version information.

HOW IT WORKS:
    apt/dpkg and snap do not honour Linux capabilities: dpkg refuses to modify
    the system unless the effective UID is 0, and snap authenticates the
    caller's UID with snapd/polkit. They require a real root UID, not a set of
    capabilities.

    So `setup` uses setcap(8) to grant this binary just:
        {caps}
    At run time the tool uses those capabilities to switch its UID/GID to 0 and
    then runs the update commands as real root. This lets an unprivileged user
    run updates without sudo, while `teardown` removes the grant again.

NOTES:
    * `setup`/`teardown` require root because changing file capabilities needs
      CAP_SETFCAP.
    * CAP_SETUID lets this binary become root on demand, so treat a binary that
      has been through `setup` as privileged. Use `teardown` to revoke it when
      no longer needed.
    * If neither setup nor sudo is used, the update commands will simply fail
      with permission errors.
",
        caps = caps::cap_names().replace(',', ", ")
    );
}
