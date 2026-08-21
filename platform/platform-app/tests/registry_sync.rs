//! Guards that the theme and components vendored by `topcoat ui` stay
//! in sync with the built-in topcoat-ui registry (the upstream pattern
//! — see `demos/coffee-shop/tests/registry_sync.rs` in the topcoat
//! repo). When a registry source changes, these tests fail until the
//! files are refreshed with the `topcoat ui` commands named in the
//! failure message.
//!
//! Two kinds of files live in `src/components/` (see
//! `src/components.rs`): vendored ones, pinned byte-for-byte to the
//! registry here, and [`OURS`] — components we wrote in the registry's
//! conventions, which the registry does not know and which must not
//! shadow a registry name.
//!
//! Unlike the upstream demo, the registry crate is not a workspace
//! member here: `cargo metadata` resolves `topcoat-ui-registry` (pulled
//! in by topcoat's `ui` feature) to its source in the cargo registry
//! cache, the same way the `topcoat ui` CLI finds it.

use std::path::{Path, PathBuf};
use std::process::Command;

use topcoat_ui::{DEFAULT_REGISTRY_CRATE, Registry};

/// This package's root, where `topcoat ui` installed the theme and
/// components.
fn package_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The directory `topcoat ui` installs components into — and where our
/// own components live too, in the same style.
fn components_dir() -> PathBuf {
    package_root().join("src/components")
}

/// The components that are ours, not the registry's: created following
/// the registry components' conventions and exempt from the sync
/// checks. Keep in step with the "Ours" list in `src/components.rs`.
const OURS: &[&str] = &["field.rs", "page_title.rs", "site_header.rs"];

/// The built-in registry, loaded from the `topcoat-ui-registry` crate's
/// source directory as resolved by `cargo metadata`.
fn registry() -> Registry {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1"])
        .current_dir(package_root())
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata output parses");
    let package = metadata["packages"]
        .as_array()
        .expect("metadata lists packages")
        .iter()
        .find(|package| package["name"] == DEFAULT_REGISTRY_CRATE)
        .expect("the ui feature pulls the built-in registry crate into the graph");
    let manifest = Path::new(
        package["manifest_path"]
            .as_str()
            .expect("packages have a manifest path"),
    );
    let subdir = package["metadata"]["topcoat-ui"]["registry"]
        .as_str()
        .expect("the registry crate declares [package.metadata.topcoat-ui].registry");
    let dir = manifest
        .parent()
        .expect("a manifest path has a parent")
        .join(subdir);
    Registry::load(dir).expect("the built-in registry loads")
}

/// Reads an installed file, failing with `hint` when it is missing.
fn read_installed(path: &PathBuf, hint: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}; {hint}", path.display()))
}

#[test]
fn theme_matches_registry() {
    let registry = registry();
    let theme = registry
        .theme("neutral")
        .expect("the registry offers the neutral theme this package installed");

    let installed = package_root().join(theme.file_name());
    let hint = "re-install the theme by deleting it along with components.toml \
        and running `topcoat ui init --theme neutral --package platform-app`";
    assert!(
        read_installed(&installed, hint) == theme.read_source().unwrap(),
        "{} no longer matches the registry's neutral theme; {hint}",
        installed.display(),
    );
}

#[test]
fn components_match_registry() {
    let registry = registry();

    for name in registry.names() {
        let component = registry.get(name).expect("name came from the registry");
        let installed = components_dir().join(component.file_name());
        if !installed.exists() {
            continue;
        }
        let hint = format!("run `topcoat ui add {name} --overwrite --package platform-app`");
        assert!(
            read_installed(&installed, &hint) == component.read_source().unwrap(),
            "{} no longer matches the registry's `{name}` component; {hint}",
            installed.display(),
        );
    }
}

#[test]
fn every_component_is_vendored_or_ours() {
    let registry = registry();
    let known: Vec<String> = registry
        .names()
        .map(|name| {
            registry
                .get(name)
                .expect("name came from the registry")
                .file_name()
                .to_owned()
        })
        .collect();

    let dir = components_dir();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()));
    for entry in entries {
        let file = entry
            .unwrap_or_else(|error| panic!("cannot read an entry of {}: {error}", dir.display()))
            .file_name();
        let file = file.to_str().expect("component file names are UTF-8");
        if OURS.contains(&file) {
            // Ours must not shadow a registry component: `topcoat ui
            // add` of that name would offer to overwrite our file.
            assert!(
                !known.iter().any(|known| known == file),
                "{} is ours but the registry now offers a component with the same \
                 file name; rename ours",
                dir.join(file).display(),
            );
            continue;
        }
        assert!(
            known.iter().any(|known| known == file),
            "{} is neither a component of the built-in registry nor listed as ours; \
             remove it with `topcoat ui remove --package platform-app`, move it out of {}, \
             or add it to OURS in tests/registry_sync.rs (and src/components.rs)",
            dir.join(file).display(),
            dir.display(),
        );
    }
}
