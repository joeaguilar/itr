fn main() {
    let describe = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string());

    // shape_version (include!d from src/version_shape.rs, where cargo test
    // covers it) keeps the version semver-shaped even when a tagless checkout
    // makes `git describe` fall back to a bare commit hash.
    let version = shape_version(describe.as_deref(), env!("CARGO_PKG_VERSION"));

    println!("cargo:rustc-env=ITR_VERSION={}", version);
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-changed=src/version_shape.rs");
}

include!("src/version_shape.rs");
