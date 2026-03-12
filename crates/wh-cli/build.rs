use std::process::Command;

fn main() {
    // Inject git commit hash at compile time (TT-06)
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=WH_GIT_HASH={git_hash}");

    // Inject target triple at compile time (TT-06)
    // TARGET is always set by cargo during builds; fallback for edge cases
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=WH_TARGET_TRIPLE={target}");

    // Re-run if HEAD changes. Use git rev-parse to find the repo root
    // so this works regardless of crate nesting depth or worktree layout.
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
    {
        if output.status.success() {
            if let Ok(git_dir) = String::from_utf8(output.stdout) {
                let git_dir = git_dir.trim();
                println!("cargo:rerun-if-changed={git_dir}/HEAD");
                println!("cargo:rerun-if-changed={git_dir}/refs/");
            }
        }
    }
}
