use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use object::{Object, ObjectSection, ObjectSymbol};

/// One test case discovered in an embedded-test ELF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedTest {
    /// Fully qualified test name: `<module_path>::<name>`.
    pub name: String,
    pub ignored: bool,
    pub should_panic: bool,
    /// Address of the test entrypoint function (Thumb bit included on ARM --
    /// pass it back to the firmware verbatim via `run_addr`).
    pub entrypoint: u64,
}

/// Metadata stored in the symbol name of every test in `.embedded_test`.
/// Unknown fields (e.g. a future `timeout` or a macro `disambiguator`) are ignored.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
struct TestMetadata {
    name: String,
    ignored: bool,
    should_panic: bool,
}

/// Discover the embedded-test cases contained in an ELF test binary.
///
/// Layout (embedded-test 0.7.x + embedded-test-macros 0.8.x):
/// * the `.embedded_test` section holds one JSON-named symbol per test, carrying
///   `{name, ignored, should_panic}`;
/// * the `EMBEDDED_TEST_VERSION` symbol marks the binary as an embedded-test binary;
/// * every test has an entrypoint function symbol `<module_path>::__<name>_entrypoint`
///   whose address is the value the firmware expects in its semihosting command line.
///
/// The 12-byte records the metadata symbols point at are NOT reliable in a linked
/// executable (the section is not PT_LOAD-backed and carries no relocations), so the
/// entrypoint address is taken from the symbol table instead.
///
/// Returns `None` when the ELF is not an embedded-test binary.
pub fn read_embedded_tests(path: &Path) -> Result<Option<Vec<EmbeddedTest>>> {
    let data = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let file = object::File::parse(&*data).context("failed to parse ELF file")?;

    let Some(section) = file.section_by_name(".embedded_test") else {
        return Ok(None);
    };
    let section_idx = section.index();

    let mut is_embedded_test = false;
    let mut metadata: Vec<TestMetadata> = Vec::new();
    let mut entrypoints: HashMap<String, u64> = HashMap::new();

    for symbol in file.symbols() {
        let Ok(raw_name) = symbol.name() else {
            continue;
        };
        if raw_name.is_empty() {
            continue;
        }
        if raw_name == "EMBEDDED_TEST_VERSION" {
            is_embedded_test = true;
            continue;
        }
        if raw_name.starts_with('{') {
            if symbol.section_index() == Some(section_idx)
                && let Ok(meta) = serde_json::from_str::<TestMetadata>(raw_name)
            {
                metadata.push(meta);
            }
            continue;
        }
        // Entrypoint functions are emitted as `<module>::__<name>_entrypoint`.
        let demangled = rustc_demangle::try_demangle(raw_name)
            .map(|d| d.to_string())
            .unwrap_or_else(|_| raw_name.to_string());
        // v0 mangling renders the crate as `name[hash]`; drop the disambiguator.
        let demangled = match demangled.find('[') {
            Some(i) if demangled[i..].starts_with('[') && demangled[i..].contains(']') => {
                let j = demangled[i..].find(']').unwrap() + i;
                format!("{}{}", &demangled[..i], &demangled[j + 1..])
            }
            _ => demangled,
        };
        if demangled.contains("::__") && demangled.ends_with("_entrypoint") {
            entrypoints.insert(demangled, symbol.address());
        }
    }

    if !is_embedded_test && metadata.is_empty() {
        return Ok(None);
    }

    let mut tests = Vec::new();
    let mut ambiguous = 0usize;
    for meta in metadata {
        let bare = if meta.name.is_empty() {
            "<unknown>"
        } else {
            &meta.name
        };
        let suffix = format!("::__{bare}_entrypoint");
        let mut candidates: Vec<(&String, &u64)> = entrypoints
            .iter()
            .filter(|(full, _)| full.ends_with(&suffix))
            .collect();
        candidates.sort();

        let (name, entrypoint) = match candidates.as_slice() {
            [(full, addr)] => {
                let module = full.strip_suffix(&suffix).unwrap();
                (qualified_name(Some(module), bare), **addr)
            }
            [] => {
                // No module-qualified entrypoint found (unusual macro version?):
                // fall back to any symbol ending in `__<name>_entrypoint`.
                let alt = format!("__{bare}_entrypoint");
                match entrypoints.iter().find(|(full, _)| full.ends_with(&alt)) {
                    Some((full, addr)) => {
                        let module = full.strip_suffix(&alt).unwrap().strip_suffix("::");
                        (qualified_name(module, bare), *addr)
                    }
                    None => {
                        eprintln!(
                            "qtest: warning: no entrypoint symbol found for test `{bare}` in {}; skipping it",
                            path.display()
                        );
                        continue;
                    }
                }
            }
            _ => {
                ambiguous += 1;
                let (full, addr) = candidates[0];
                let module = full.strip_suffix(&suffix).unwrap();
                (qualified_name(Some(module), bare), *addr)
            }
        };
        tests.push(EmbeddedTest {
            name,
            ignored: meta.ignored,
            should_panic: meta.should_panic,
            entrypoint,
        });
    }

    if ambiguous > 0 {
        eprintln!(
            "qtest: warning: {ambiguous} test(s) in {} have entrypoint name collisions across modules; verify results",
            path.display()
        );
    }

    tests.sort_by_key(|t| t.entrypoint);
    Ok(Some(tests))
}

fn qualified_name(module_path: Option<&str>, name: &str) -> String {
    match module_path {
        Some(module) if !module.is_empty() => format!("{module}::{name}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_json() {
        let meta: TestMetadata = serde_json::from_str(
            r#"{"name":"can_connect_sensor","ignored":false,"should_panic":false}"#,
        )
        .unwrap();
        assert_eq!(meta.name, "can_connect_sensor");
        assert!(!meta.ignored);
        assert!(!meta.should_panic);
    }

    #[test]
    fn metadata_ignores_unknown_fields() {
        let meta: TestMetadata =
            serde_json::from_str(r#"{"name":"t","timeout":10,"ignored":true,"disambiguator":0}"#)
                .unwrap();
        assert!(meta.ignored);
        assert!(!meta.should_panic);
    }

    #[test]
    fn qualifies_name_with_module_path() {
        assert_eq!(qualified_name(Some("app::tests"), "t"), "app::tests::t");
        assert_eq!(qualified_name(Some(""), "t"), "t");
        assert_eq!(qualified_name(None, "t"), "t");
    }
}
