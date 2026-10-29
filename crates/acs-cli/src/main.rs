//! The `acs` binary: commands and hook entrypoints.

/// Returns the version banner printed by `acs --version`.
fn version_banner() -> String {
    format!("acs {}", env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{}", version_banner());
}

#[cfg(test)]
mod tests {
    use super::version_banner;

    #[test]
    fn banner_has_name_and_version() {
        assert!(version_banner().starts_with("acs "));
    }
}
