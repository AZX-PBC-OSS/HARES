//! Workspace-wide control-gate guard.
//!
//! `control_gate_guard.rs` proves no `impl Equipment` in this crate's `src`
//! overrides the gated dispatch boundary. The `Equipment` trait is public:
//! any sibling crate can implement it, and an override there would silently
//! drop the central `ControlSignal::validate_numeric_bounds()` enforcement
//! for that equipment — the same class the local guard closes inside this
//! crate. This guard scans every sibling crate's `src` and `tests` for the
//! same violation, so the contract — every path that delivers a signal
//! enforces the central bounds before the arm runs — holds workspace-wide,
//! not only where the first guard looks.

use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

const GATED_METHODS: [&str; 2] = ["apply_control", "apply_control_unchecked"];

fn collect_rs_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read_dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Flags any `impl … Equipment … for …` block that defines a gated method.
#[derive(Default)]
struct GateOverrideCollector {
    violations: Vec<String>,
}

impl<'ast> Visit<'ast> for GateOverrideCollector {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let implements_equipment = node
            .trait_
            .as_ref()
            .and_then(|(_, path, _)| path.segments.last())
            .is_some_and(|seg| seg.ident == "Equipment");
        if implements_equipment {
            for item in &node.items {
                if let syn::ImplItem::Fn(method) = item {
                    let name = method.sig.ident.to_string();
                    if GATED_METHODS.contains(&name.as_str()) {
                        let target = match &*node.self_ty {
                            syn::Type::Path(p) => p
                                .path
                                .segments
                                .last()
                                .map(|s| s.ident.to_string())
                                .unwrap_or_else(|| "<type>".to_string()),
                            _ => "<type>".to_string(),
                        };
                        self.violations.push(format!(
                            "`impl Equipment for {target}` overrides `{name}` — \
                             implement `apply_signal` instead; the gate and central \
                             numeric-bounds enforcement live in the default body"
                        ));
                    }
                }
            }
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// The guard must not be vacuous: prove the collector flags a synthetic
/// override and ignores a compliant impl plus an inherent method of the
/// same name (the pattern used by the heat-pump heater).
#[test]
fn guard_flags_synthetic_override_and_ignores_compliant_code() {
    let violating = syn::parse_file(
        "impl Equipment for Bad { fn apply_control_unchecked(&mut self, s: &ControlSignal) -> Result<()> { Ok(()) } }",
    )
    .unwrap();
    let mut collector = GateOverrideCollector::default();
    collector.visit_file(&violating);
    assert_eq!(collector.violations.len(), 1);

    let compliant = syn::parse_file(
        "impl Equipment for Good { fn apply_signal(&mut self, s: &ControlSignal) -> Result<()> { Ok(()) } } \n\
         impl Heater { fn apply_control_unchecked(&mut self, s: &ControlSignal) -> Result<()> { Ok(()) } }",
    )
    .unwrap();
    let mut collector = GateOverrideCollector::default();
    collector.visit_file(&compliant);
    assert!(collector.violations.is_empty());
}

#[test]
fn no_sibling_crate_equipment_impl_overrides_the_gated_control_boundary() {
    let crates_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut crate_dirs: Vec<PathBuf> = fs::read_dir(&crates_root)
        .expect("read crates directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    crate_dirs.sort();
    assert!(
        crate_dirs.len() > 1,
        "guard found no sibling crates under {crates_root:?} — is it scanning the right tree?"
    );

    let mut violations = Vec::new();
    let mut files_scanned = 0usize;
    for crate_dir in &crate_dirs {
        for sub in ["src", "tests"] {
            let root = crate_dir.join(sub);
            if !root.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            collect_rs_files(&root, &mut files);
            for file in files {
                files_scanned += 1;
                let content = fs::read_to_string(&file).expect("read source file");
                let parsed = syn::parse_file(&content)
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", file.display()));
                let mut collector = GateOverrideCollector::default();
                collector.visit_file(&parsed);
                for v in collector.violations {
                    violations.push(format!("{}: {v}", file.display()));
                }
            }
        }
    }

    assert!(
        files_scanned > 0,
        "guard scanned no files under {crates_root:?} — is it scanning the right tree?"
    );
    assert!(
        violations.is_empty(),
        "workspace control-gate guard failed:\n{}",
        violations.join("\n")
    );
}
