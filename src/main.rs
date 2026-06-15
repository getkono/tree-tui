//! tree-tui — a ratatui-based terminal user interface.

use color_eyre::Result;

/// Returns the application's startup banner.
fn greeting() -> &'static str {
    "tree-tui — terminal UI"
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    tracing::info!("starting tree-tui");
    println!("{}", greeting());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_stable() {
        assert_eq!(greeting(), "tree-tui — terminal UI");
    }
}
