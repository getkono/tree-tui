//! Modular data collectors and the lazy lens-computation entry point.
//!
//! Each collector is an independent data source keyed by relative path:
//! [`walk`] (skeleton + size, run eagerly at startup) and [`code`] (tokei, run
//! lazily). Git collectors arrive in a later phase. [`compute`] runs the
//! collector for one lens on a background thread; the app aggregates the result
//! into a cached [`Layer`](crate::model::Layer).

mod code;
mod git;
mod walk;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::model::{ChurnData, CodeData, Lens, StatusData};

pub use git::{head_short_hash, is_repo};
pub use walk::walk;

/// The per-file data produced by computing one lens, tagged by which lens it is
/// for. Aggregated into a [`Layer`](crate::model::Layer) by the app.
pub enum LayerResult {
    Code {
        files: HashMap<PathBuf, CodeData>,
        inaccurate: bool,
    },
    Churn(HashMap<PathBuf, ChurnData>),
    Status(HashMap<PathBuf, StatusData>),
}

impl LayerResult {
    /// Which lens this result populates.
    pub fn lens(&self) -> Lens {
        match self {
            LayerResult::Code { .. } => Lens::Code,
            LayerResult::Churn(_) => Lens::Churn,
            LayerResult::Status(_) => Lens::Status,
        }
    }
}

/// Compute a lens's per-file data. Called from a blocking thread.
///
/// `Size` reads the always-present node bytes and never reaches here. Git lenses
/// degrade to empty maps outside a repository (and are unavailable there anyway).
pub fn compute(lens: Lens, root: &Path) -> LayerResult {
    match lens {
        Lens::Code => {
            let (files, inaccurate) = code::collect_code(root);
            LayerResult::Code { files, inaccurate }
        }
        Lens::Churn => LayerResult::Churn(git::churn(root)),
        Lens::Status => LayerResult::Status(git::status(root)),
        Lens::Size => unreachable!("the size lens reads node bytes; it never computes a layer"),
    }
}

/// `path` relative to `root`, reduced to its normal components. Falls back to the
/// path's own normal components if it is not under `root`.
pub(crate) fn relative_path(path: &Path, root: &Path) -> PathBuf {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect()
}
