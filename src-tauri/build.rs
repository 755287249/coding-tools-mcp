use std::path::PathBuf;
use std::process::Command;

fn git_output(manifest_dir: &PathBuf, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_clean(manifest_dir: &PathBuf) -> Option<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    output.status.success().then(|| output.stdout.is_empty())
}

fn tool_evolution_proposal_ids(message: &str) -> Vec<String> {
    message
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Tool-Evolution-Proposal:"))
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| {
            value.len() == 64 && value.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit())
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(16)
        .collect()
}

fn main() {
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let build_git_sha =
        git_output(&manifest_dir, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CTMCP_BUILD_GIT_SHA={build_git_sha}");
    let build_source_clean = git_clean(&manifest_dir)
        .map(|clean| if clean { "true" } else { "false" })
        .unwrap_or("unknown");
    println!("cargo:rustc-env=CTMCP_BUILD_SOURCE_CLEAN={build_source_clean}");
    let proposal_ids = if build_source_clean == "true" {
        git_output(&manifest_dir, &["log", "-1", "--pretty=%B"])
            .map(|message| tool_evolution_proposal_ids(&message))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    println!(
        "cargo:rustc-env=CTMCP_TOOL_EVOLUTION_PROPOSAL_IDS={}",
        proposal_ids.join(",")
    );

    if let Some(git_head_path) = git_output(&manifest_dir, &["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={git_head_path}");
    }
    if let Some(head_ref) = git_output(&manifest_dir, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(ref_path) = git_output(&manifest_dir, &["rev-parse", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={ref_path}");
        }
    }

    #[cfg(feature = "desktop")]
    tauri_build::build();
}
