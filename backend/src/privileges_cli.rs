//! `pack privileges status|install` — file capability install path for operators.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::linux_privileges;

const SETCAP_CAPS: &str = "cap_net_raw,cap_net_admin+eip";

pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        None | Some("status") => status(),
        Some("install") => install(),
        Some(other) => bail!("unknown subcommand {other:?}; use: pack privileges status|install"),
    }
}

fn pack_exe_path() -> Result<PathBuf> {
    let link = std::fs::read_link("/proc/self/exe").context("read /proc/self/exe")?;
    Ok(link)
}

fn status() -> Result<()> {
    let exe = pack_exe_path()?;
    println!("pack binary: {}", exe.display());
    println!("running as root: {}", linux_privileges::running_as_root());
    println!(
        "CAP_NET_RAW effective: {}",
        linux_privileges::net_raw_effective()
    );
    println!(
        "CAP_NET_ADMIN permitted: {}",
        linux_privileges::net_admin_permitted()
    );
    println!("WiFi control: iw/ip subprocesses (channel hop)");
    if let Ok(out) = Command::new("getcap").arg(&exe).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.trim().is_empty() {
            println!("file capabilities: (none)");
        } else {
            println!("file capabilities: {}", s.trim());
        }
    }
    if !linux_privileges::running_as_root()
        && (!linux_privileges::net_raw_effective() || !linux_privileges::net_admin_permitted())
    {
        println!();
        println!("Install (copy/paste):");
        println!(
            "  sudo setcap {} \"$(readlink -f {})\"",
            SETCAP_CAPS,
            exe.display()
        );
        println!("Or: pack privileges install");
    }
    Ok(())
}

fn install() -> Result<()> {
    let exe = pack_exe_path()?;
    let exe = fs_canonical(&exe)?;
    if linux_privileges::running_as_root() {
        run_setcap(&exe)?;
        status()?;
        return Ok(());
    }
    if Command::new("pkexec").arg("--version").output().is_ok() {
        let status = Command::new("pkexec")
            .args(["setcap", SETCAP_CAPS, exe.to_str().unwrap_or_default()])
            .status()
            .context("pkexec setcap")?;
        if !status.success() {
            bail!("pkexec setcap failed with {status}");
        }
    } else {
        let status = Command::new("sudo")
            .args([
                "setcap",
                SETCAP_CAPS,
                exe.to_str().context("non-UTF8 exe path")?,
            ])
            .status()
            .context("sudo setcap")?;
        if !status.success() {
            bail!("sudo setcap failed with {status}");
        }
    }
    status()?;
    Ok(())
}

fn fs_canonical(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path).with_context(|| format!("canonicalize {}", path.display()))
}

fn run_setcap(exe: &Path) -> Result<()> {
    let status = Command::new("setcap")
        .args([SETCAP_CAPS, exe.to_str().context("non-UTF8 exe path")?])
        .status()
        .context("setcap")?;
    if !status.success() {
        bail!("setcap failed with {status}");
    }
    Ok(())
}
