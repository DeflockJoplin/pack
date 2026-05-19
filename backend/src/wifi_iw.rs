//! `iw` / `ip` subprocess backend for WiFi channel control and read-only discovery.

use std::process::Command;

use anyhow::Context;

fn run_output(cmd: &str, args: &[&str]) -> anyhow::Result<(bool, String, String)> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("spawn {cmd}"))?;
    let ok = out.status.success();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Ok((ok, stdout, stderr))
}

pub fn set_channel(name: &str, ch: u8) -> anyhow::Result<()> {
    let (ok, _, stderr) = run_output(
        "iw",
        &["dev", name, "set", "channel", &ch.to_string(), "HT20"],
    )?;
    if ok {
        Ok(())
    } else {
        anyhow::bail!(
            "{}",
            if stderr.is_empty() {
                format!("iw set channel failed (exit)")
            } else {
                stderr
            }
        )
    }
}
