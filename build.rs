//! Capture build-time metadata for the `-V` report, with zero extra dependencies.
//!
//! Emits `cargo:rustc-env=TREE_*` values that `src/version.rs` reads back via
//! `env!`. Git information degrades gracefully to `"unknown"` when building
//! outside a git checkout (e.g. `cargo install` from a packaged crate).

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Re-run when HEAD/index move so the commit hash and dirty flag stay current.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let sha = git(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = match git(&["status", "--porcelain"]) {
        Some(out) => !out.is_empty(),
        None => false,
    };
    emit("TREE_GIT_SHA", &sha);
    emit("TREE_GIT_DIRTY", if dirty { "true" } else { "false" });

    emit("TREE_BUILD_TIME", &build_time());
    emit(
        "TREE_BUILD_PROFILE",
        &std::env::var("PROFILE").unwrap_or_default(),
    );
    emit(
        "TREE_BUILD_TARGET",
        &std::env::var("TARGET").unwrap_or_default(),
    );

    let (rustc, channel) = rustc_info();
    emit("TREE_RUSTC", &rustc);
    emit("TREE_RUST_CHANNEL", &channel);
}

fn emit(key: &str, val: &str) {
    println!("cargo:rustc-env={key}={val}");
}

/// Run a git subcommand, returning trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `(version, channel)` from the active rustc, e.g. `("rustc 1.96.0", "stable")`.
fn rustc_info() -> (String, String) {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let text = match Command::new(&rustc).arg("-vV").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return ("unknown".to_string(), "unknown".to_string()),
    };
    let release = text
        .lines()
        .find_map(|l| l.strip_prefix("release:"))
        .map(str::trim)
        .unwrap_or("");
    let channel = if release.contains("nightly") {
        "nightly"
    } else if release.contains("beta") {
        "beta"
    } else {
        "stable"
    };
    let version = if release.is_empty() {
        "unknown".to_string()
    } else {
        format!("rustc {release}")
    };
    (version, channel.to_string())
}

/// Build time as an RFC 3339 UTC string, honoring `SOURCE_DATE_EPOCH`.
fn build_time() -> String {
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    format_rfc3339_utc(epoch)
}

/// Convert a Unix timestamp to `YYYY-MM-DDThh:mm:ssZ` (proleptic Gregorian).
///
/// Uses Howard Hinnant's `civil_from_days` algorithm — no date dependency.
fn format_rfc3339_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (hour, minute, second) = (secs / 3_600, (secs % 3_600) / 60, secs % 60);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day of era, [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}
