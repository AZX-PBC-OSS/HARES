//! Control-gate guard — the gated dispatch boundary must not be overridden.
//!
//! `Equipment::apply_control` and `Equipment::apply_control_unchecked`
//! (`src/lib.rs`) carry the capability gate and the central
//! `ControlSignal::validate_numeric_bounds()` enforcement with default trait
//! bodies, delegating to the equipment-specific `apply_signal` arm. The
//! contract — every path that delivers a signal enforces the central bounds
//! before the arm runs — holds only while no implementor overrides either
//! method. The doc comment says "do not override"; this guard makes the rule
//! fail loudly instead of relying on every future reader honouring a comment.
//!
//! Mirrors the AST-scan approach of `si_guard.rs`: parse each source file
//! with `syn` and inspect `impl Equipment for …` blocks, so comments and
//! string literals cannot produce false positives.

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
/// override and ignores both a compliant impl and an inherent method of the
/// same name (the pattern used by the heat-pump heater).
#[test]
fn guard_flags_synthetic_override_and_ignores_compliant_code() {
    let violating = syn::parse_file(
        "impl Equipment for Bad { fn apply_control(&mut self, s: &ControlSignal) -> Result<()> { Ok(()) } }",
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
fn no_equipment_impl_overrides_the_gated_control_boundary() {
    let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs_files(&src_root, &mut files);
    assert!(
        !files.is_empty(),
        "guard found no sources under {src_root:?} — is it scanning the right tree?"
    );

    let mut violations = Vec::new();
    for file in &files {
        let content = fs::read_to_string(file).expect("read source file");
        let parsed = syn::parse_file(&content)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", file.display()));
        let mut collector = GateOverrideCollector::default();
        collector.visit_file(&parsed);
        for v in collector.violations {
            let rel = file
                .strip_prefix(&src_root)
                .expect("strip prefix")
                .to_string_lossy()
                .replace('\\', "/");
            violations.push(format!("{rel}: {v}"));
        }
    }

    assert!(
        violations.is_empty(),
        "control-gate guard failed:\n{}",
        violations.join("\n")
    );
}
