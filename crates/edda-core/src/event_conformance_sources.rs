//! Source-derived producer inventory. A dynamic producer requires explicit audit.
use std::collections::BTreeSet;
use std::path::Path;
use syn::visit::{self, Visit};
use syn::{Expr, Member};

#[derive(Default)]
struct Producers {
    types: BTreeSet<String>,
    file: String,
    function: String,
}

fn literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(e) => match &e.lit {
            syn::Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        Expr::MethodCall(e)
            if matches!(
                e.method.to_string().as_str(),
                "to_string" | "to_owned" | "into"
            ) =>
        {
            literal(&e.receiver)
        }
        Expr::Call(e) if e.args.len() == 1 => literal(&e.args[0]),
        _ => None,
    }
}

impl Producers {
    fn record(&mut self, value: &Expr) {
        if let Some(name) = literal(value) {
            self.types.insert(name);
            return;
        }
        // These are readers (not type issuers), or the one shared task builder
        // whose callers are audited below. Keep the exception at function scope.
        let passthrough = matches!(
            (self.file.as_str(), self.function.as_str()),
            ("edda-core/src/event.rs", "build_task_event")
                | ("edda-ledger/src/sqlite_store/mappers.rs", "row_to_event")
                | ("edda-ledger/src/sqlite_store/mappers.rs", "map_event_row")
                | (
                    "edda-ledger/src/sqlite_store/events.rs",
                    "events_after_rowid"
                )
        );
        assert!(
            passthrough,
            "unclassified dynamic Event producer: {}::{}",
            self.file, self.function
        );
    }
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr
                    .parse_args::<syn::Path>()
                    .is_ok_and(|p| p.is_ident("test")))
    })
}

impl<'ast> Visit<'ast> for Producers {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !test_only(&item.attrs) {
            visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if test_only(&item.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.function = previous;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if test_only(&item.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, item.sig.ident.to_string());
        visit::visit_impl_item_fn(self, item);
        self.function = previous;
    }

    fn visit_expr_struct(&mut self, expr: &'ast syn::ExprStruct) {
        // Recognize qualified Event and aliases retaining the envelope fields.
        let envelope = expr
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "Event")
            || expr
                .fields
                .iter()
                .any(|f| matches!(&f.member, Member::Named(n) if n == "parent_hash"));
        if envelope {
            for field in &expr.fields {
                if matches!(&field.member, Member::Named(name) if name == "event_type") {
                    self.record(&field.expr);
                }
            }
        }
        visit::visit_expr_struct(self, expr);
    }

    fn visit_expr_assign(&mut self, expr: &'ast syn::ExprAssign) {
        if matches!(&*expr.left, Expr::Field(f) if matches!(&f.member, Member::Named(n) if n == "event_type"))
        {
            self.record(&expr.right);
        }
        visit::visit_expr_assign(self, expr);
    }

    fn visit_expr_call(&mut self, expr: &'ast syn::ExprCall) {
        if matches!(&*expr.func, Expr::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "build_task_event"))
        {
            assert_eq!(expr.args.len(), 4);
            self.record(&expr.args[2]);
        }
        visit::visit_expr_call(self, expr);
    }
}

pub(super) fn inventory(crates: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, base: &Path, scanner: &mut Producers) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "tests" || n == "target")
                {
                    continue;
                }
                walk(&path, base, scanner);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path.file_name().unwrap().to_string_lossy();
                if name == "tests.rs" || name.starts_with("event_conformance") {
                    continue;
                }
                scanner.file = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let source = std::fs::read_to_string(&path).unwrap();
                scanner.visit_file(&syn::parse_file(&source).unwrap());
            }
        }
    }
    let mut scanner = Producers::default();
    walk(crates, crates, &mut scanner);
    scanner.types
}

#[test]
fn inventory_finds_qualified_constructors_mutation_and_task_calls() {
    let source = r#"fn producer() {
        let e = edda_core::types::Event { event_type: "brand_new".into() };
        e.event_type = "mutated".to_string();
        build_task_event("main", None, "task.new", payload);
    }"#;
    let mut scanner = Producers::default();
    scanner.visit_file(&syn::parse_file(source).unwrap());
    assert_eq!(
        scanner.types,
        ["brand_new", "mutated", "task.new"]
            .map(str::to_owned)
            .into()
    );
}

#[test]
#[should_panic(expected = "unclassified dynamic Event producer")]
fn inventory_fails_closed_on_new_dynamic_constructor() {
    let mut scanner = Producers::default();
    scanner.visit_file(
        &syn::parse_file("fn producer(t: String) { Event { event_type: t } }").unwrap(),
    );
}
