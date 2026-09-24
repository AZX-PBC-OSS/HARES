//! Identity-setter call-site guard.
//!
//! `Equipment::set_equipment_id` is the dwelling-mediated identity channel:
//! `Dwelling::add_equipment` / `Dwelling::replace_equipment` are the only
//! production callers authorized to invoke it, because they pair the write
//! with collision validation, the landed-write postcondition, and the
//! never-reused id counter. A direct call from anywhere else in workspace
//! `src` would mutate equipment identity outside that contract — an
//! equipment's id could drift after registration, desyncing
//! `equipment_id_by_name` from the live descriptor and reintroducing the
//! I-06 misbinding (every id-keyed lookup addressing the wrong equipment).
//!
//! This guard is a **heuristic**, not a decision like its three siblings
//! (`si_guard`, `control_gate_guard`, `workspace_gate_guard`, which scan for
//! syntactically unambiguous `impl Equipment` method *definitions*): `syn`
//! is purely syntactic, so a name-matched scan of method *call sites* can
//! false-positive on same-named methods on unrelated types (resolved by
//! renaming) and false-negative on UFCS or macro-generated calls (covered
//! by the other layers: the entrance postcondition, the guard-checked
//! setter, and the debug desync invariant). Its failure mode is a loud CI
//! error, which is why it is worth keeping despite the heuristic edge. Its
//! reach is workspace `src` only — an in-repo test cannot scan out-of-tree
//! crates, and out-of-tree pre-registration use of the setter on one's own
//! equipment is harmless: whatever id is present at `add_equipment` is
//! validated by the postcondition (explicit or assigned).

use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::Visit;

/// Files authorized to contain `set_equipment_id` call sites in workspace
/// `src`:
/// - `hares-equipment/src/macros.rs` — the `delegate_equipment!` forwarding;
/// - `hares-core/src/dwelling/mod.rs` — `Dwelling`'s entrance logic
///   (`assign_equipment_identity`), the authorized caller.
const AUTHORIZED_FILES: [&str; 2] = [
    "hares-equipment/src/macros.rs",
    "hares-core/src/dwelling/mod.rs",
];

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

fn is_authorized(path: &Path) -> bool {
    AUTHORIZED_FILES.iter().any(|suffix| path.ends_with(suffix))
}

/// Flags every `expr.set_equipment_id(..)` method call in a file that is not
/// authorized to make one.
#[derive(Default)]
struct IdentityCallCollector {
    violations: Vec<String>,
}

impl<'ast> Visit<'ast> for IdentityCallCollector {
    /// Skip `#[cfg(test)]` modules entirely: test code may exercise the
    /// setter directly (the macro-forwarding regression does), and the
    /// guard's rule concerns production call sites only.
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let is_test_module = node.attrs.iter().any(|attr| {
            attr.path().is_ident("cfg")
                && matches!(&attr.meta, syn::Meta::List(list) if list.tokens.to_string().contains("test"))
        });
        if !is_test_module {
            syn::visit::visit_item_mod(self, node);
        }
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "set_equipment_id" {
            self.violations.push(
                "call to `set_equipment_id` — the dwelling \
                 (`add_equipment`/`replace_equipment`) is the only authorized \
                 caller; it validates collisions, verifies the write landed, \
                 and advances the id counter around it"
                    .to_string(),
            );
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// The guard must not be vacuous: prove the collector flags a synthetic
/// unauthorized call, and ignores a trait *declaration* (the method
/// signature itself) plus an authorized-context forwarding call.
#[test]
fn guard_flags_synthetic_call_and_ignores_declarations() {
    let violating =
        syn::parse_file("fn f(eq: &mut Ev) { eq.set_equipment_id(EquipmentId(3)); }").unwrap();
    let mut collector = IdentityCallCollector::default();
    collector.visit_file(&violating);
    assert_eq!(
        collector.violations.len(),
        1,
        "an unauthorized call site must be flagged"
    );

    let compliant = syn::parse_file(
        "trait Equipment { fn set_equipment_id(&mut self, id: EquipmentId) -> Result<()>; }",
    )
    .unwrap();
    let mut collector = IdentityCallCollector::default();
    collector.visit_file(&compliant);
    assert!(
        collector.violations.is_empty(),
        "the trait declaration itself is not a call site"
    );

    let test_module = syn::parse_file(
        "#[cfg(test)] mod tests { fn f(eq: &mut Ev) { eq.set_equipment_id(EquipmentId(3)); } }",
    )
    .unwrap();
    let mut collector = IdentityCallCollector::default();
    collector.visit_file(&test_module);
    assert!(
        collector.violations.is_empty(),
        "call sites inside #[cfg(test)] modules are test code, not production \
         callers — the guard exempts them"
    );
}

#[test]
fn no_unauthorized_identity_setter_call_sites_in_workspace_src() {
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
        let src = crate_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rs_files(&src, &mut files);
        for file in files {
            files_scanned += 1;
            let content = fs::read_to_string(&file).expect("read source file");
            let parsed = syn::parse_file(&content)
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", file.display()));
            let mut collector = IdentityCallCollector::default();
            collector.visit_file(&parsed);
            if !collector.violations.is_empty() && !is_authorized(&file) {
                for v in &collector.violations {
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
        "identity-setter guard failed:\n{}",
        violations.join("\n")
    );
}
