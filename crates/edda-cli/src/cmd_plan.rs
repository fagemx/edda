use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

// ── Scan ──

pub fn scan(cwd: &Path, purpose: Option<&str>) -> anyhow::Result<()> {
    let mut detected = Vec::new();

    // ── Rust workspace (enriched path) ──
    if cwd.join("Cargo.toml").exists() {
        detected.push("Rust");
        if let Some(members) = detect_workspace_members(cwd) {
            let crate_infos: Vec<CrateInfo> = members
                .iter()
                .filter_map(|m| {
                    let dir = find_member_dir(cwd, m);
                    dir.map(|d| parse_crate_info(&d, m))
                })
                .collect();

            if !crate_infos.is_empty() {
                print_enriched_rust_plan(cwd, &crate_infos, purpose);
                return Ok(());
            }
        }
        // Non-workspace Rust project — fall through to generic
    }

    // ── Generic language detection (unchanged) ──
    let mut phases = Vec::new();

    if detected.contains(&"Rust") {
        phases.push(Phase {
            id: "core".into(),
            prompt: "Implement core functionality".into(),
            depends_on: vec![],
        });
        phases.push(Phase {
            id: "tests".into(),
            prompt: "Add tests for all modules".into(),
            depends_on: vec!["core".into()],
        });
    }

    if cwd.join("package.json").exists() {
        detected.push("Node.js");
        if phases.is_empty() {
            phases.push(Phase {
                id: "setup".into(),
                prompt: "Set up project dependencies and configuration".into(),
                depends_on: vec![],
            });
            phases.push(Phase {
                id: "features".into(),
                prompt: "Implement main features".into(),
                depends_on: vec!["setup".into()],
            });
            phases.push(Phase {
                id: "tests".into(),
                prompt: "Add tests".into(),
                depends_on: vec!["features".into()],
            });
        }
    }

    if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
        detected.push("Python");
        if phases.is_empty() {
            phases.push(Phase {
                id: "setup".into(),
                prompt: "Set up Python project structure".into(),
                depends_on: vec![],
            });
            phases.push(Phase {
                id: "core".into(),
                prompt: "Implement core functionality".into(),
                depends_on: vec!["setup".into()],
            });
            phases.push(Phase {
                id: "tests".into(),
                prompt: "Add tests".into(),
                depends_on: vec!["core".into()],
            });
        }
    }

    if cwd.join("go.mod").exists() {
        detected.push("Go");
        if phases.is_empty() {
            phases.push(Phase {
                id: "core".into(),
                prompt: "Implement core packages".into(),
                depends_on: vec![],
            });
            phases.push(Phase {
                id: "tests".into(),
                prompt: "Add tests".into(),
                depends_on: vec!["core".into()],
            });
        }
    }

    // Check for infra files
    if cwd.join("docker-compose.yml").exists() || cwd.join("Dockerfile").exists() {
        detected.push("Docker");
    }
    if cwd.join("migrations").is_dir() {
        detected.push("Database migrations");
    }

    if detected.is_empty() {
        println!("# No recognized project markers found.");
        println!("# Try: edda plan init minimal");
        println!();
        print!("{}", TEMPLATE_MINIMAL);
        return Ok(());
    }

    // Build YAML output (generic path)
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let name = sanitize_id(dir_name);

    println!("# Detected: {}", detected.join(", "));
    println!("# Suggested plan (pipe to file: edda plan scan > plan.yaml)");
    println!();
    println!("name: {name}");
    if let Some(p) = purpose {
        println!("purpose: \"{}\"", escape_yaml_str(p));
    } else {
        println!("purpose: \"TODO: describe your intent here\"");
    }
    println!();
    println!("phases:");
    for p in &phases {
        println!("  - id: {}", p.id);
        println!("    prompt: |");
        println!("      {}", p.prompt);
        if !p.depends_on.is_empty() {
            println!("    depends_on: [{}]", p.depends_on.join(", "));
        }
    }

    Ok(())
}

// ── Enriched Rust workspace output ──

fn print_enriched_rust_plan(cwd: &Path, crates: &[CrateInfo], purpose: Option<&str>) {
    let layers = compute_layers(crates);
    let layer_names: Vec<&str> = layers
        .iter()
        .map(|(_, infos)| {
            // Pick the most representative role for this layer
            infos
                .first()
                .map(|c| c.role)
                .unwrap_or("unknown")
        })
        .collect();

    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let name = sanitize_id(dir_name);

    // Header
    println!(
        "# Detected: Rust workspace ({} crates)",
        crates.len()
    );
    println!(
        "# Layers: {}",
        dedup_layer_names(&layer_names)
    );
    println!("# Edit prompts, then run: edda conduct run plan.yaml");
    println!();
    println!("name: {name}");
    if let Some(p) = purpose {
        println!("purpose: \"{}\"", escape_yaml_str(p));
    } else {
        println!("purpose: \"TODO: describe your intent here\"");
    }
    println!();
    println!("phases:");

    for (layer_idx, layer_crates) in &layers {
        // Layer comment
        let layer_role = layer_crates
            .first()
            .map(|c| c.role)
            .unwrap_or("unknown");
        println!(
            "  # ── Layer {}: {} ──",
            layer_idx, layer_role
        );

        for ci in layer_crates {
            println!("  - id: {}", ci.name);

            // Build prompt
            println!("    prompt: |");
            if ci.is_binary {
                if ci.has_clap {
                    println!("      [cli] {} — binary (clap)", ci.name);
                } else {
                    println!("      [binary] {}", ci.name);
                }
            } else if ci.internal_deps.is_empty() {
                println!("      [foundation] {} — no internal deps", ci.name);
            } else {
                println!(
                    "      [{}] {} — depends on {}",
                    ci.role,
                    ci.name,
                    ci.internal_deps.join(", ")
                );
            }

            if !ci.modules.is_empty() {
                println!("      Modules: {}", ci.modules.join(", "));
            }

            println!("      TODO: describe what to implement or change");

            // depends_on
            if !ci.internal_deps.is_empty() {
                println!("    depends_on: [{}]", ci.internal_deps.join(", "));
            }

            // checks
            println!("    check:");
            println!(
                "      - cmd_succeeds: \"cargo check -p {}\"",
                ci.name
            );
            if ci.has_tests {
                println!(
                    "      - cmd_succeeds: \"cargo test -p {}\"",
                    ci.name
                );
            }

            println!();
        }
    }
}

// ── Init ──

pub fn init(_cwd: &Path, template: Option<&str>, output: &str) -> anyhow::Result<()> {
    let Some(name) = template else {
        println!("Available templates:");
        println!();
        println!("  rust-cli     Rust CLI tool (scaffold → core → tests → docs)");
        println!("  rust-lib     Rust library (scaffold → api → tests → docs)");
        println!("  python-api   FastAPI REST API (scaffold → endpoints → tests → docs)");
        println!("  node-app     Node.js application (scaffold → features → tests → docs)");
        println!("  fullstack    Full-stack app (db → api → frontend → integration)");
        println!("  minimal      Single phase starter");
        println!();
        println!("Usage: edda plan init <template> [-o plan.yaml]");
        return Ok(());
    };

    let content = match name {
        "rust-cli" => TEMPLATE_RUST_CLI,
        "rust-lib" => TEMPLATE_RUST_LIB,
        "python-api" => TEMPLATE_PYTHON_API,
        "node-app" => TEMPLATE_NODE_APP,
        "fullstack" => TEMPLATE_FULLSTACK,
        "minimal" => TEMPLATE_MINIMAL,
        _ => {
            anyhow::bail!(
                "Unknown template: \"{name}\". Run `edda plan init` to see available templates."
            );
        }
    };

    let path = Path::new(output);
    if path.exists() {
        anyhow::bail!("{output} already exists. Remove it first or use -o to specify a different path.");
    }

    std::fs::write(path, content)?;
    println!("Created {output} from template \"{name}\"");
    println!("Edit the purpose and prompts, then run: edda conduct run {output}");
    Ok(())
}

// ── Crate enrichment ──

struct CrateInfo {
    name: String,
    internal_deps: Vec<String>,
    modules: Vec<String>,
    is_binary: bool,
    has_clap: bool,
    has_tests: bool,
    role: &'static str,
}

/// Find the actual directory for a workspace member.
/// Members can be paths like "crates/edda-core" or glob patterns.
fn find_member_dir(cwd: &Path, member_name: &str) -> Option<std::path::PathBuf> {
    // Try direct: crates/<name>
    let direct = cwd.join("crates").join(member_name);
    if direct.join("Cargo.toml").exists() {
        return Some(direct);
    }
    // Try root level
    let root_level = cwd.join(member_name);
    if root_level.join("Cargo.toml").exists() {
        return Some(root_level);
    }
    None
}

fn parse_crate_info(crate_dir: &Path, name: &str) -> CrateInfo {
    let cargo_content = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap_or_default();
    let lib_content = std::fs::read_to_string(crate_dir.join("src/lib.rs")).unwrap_or_default();
    let is_binary = crate_dir.join("src/main.rs").exists();
    let has_clap = cargo_content.contains("clap");

    let internal_deps = parse_internal_deps(&cargo_content);
    let modules = parse_modules(&lib_content);
    let has_tests = crate_dir.join("tests").is_dir() || has_inline_tests(crate_dir);

    let role = classify_role(&internal_deps, is_binary, has_clap);

    CrateInfo {
        name: name.to_string(),
        internal_deps,
        modules,
        is_binary,
        has_clap,
        has_tests,
        role,
    }
}

/// Extract internal workspace deps (edda-*) from Cargo.toml content.
/// Handles: `edda-core = { path = "..." }` and `edda-core.workspace = true`
fn parse_internal_deps(cargo_content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    let mut in_deps_section = false;

    for line in cargo_content.lines() {
        let trimmed = line.trim();

        // Track [dependencies] section
        if trimmed.starts_with('[') {
            in_deps_section = trimmed == "[dependencies]";
            continue;
        }

        if !in_deps_section {
            continue;
        }

        // Match lines starting with edda-
        // Format 1: edda-core = { path = "..." }
        // Format 2: edda-core.workspace = true
        if let Some(name) = trimmed.split(['=', '.']).next() {
            let name = name.trim();
            if name.starts_with("edda-") {
                deps.push(name.to_string());
            }
        }
    }

    deps.sort();
    deps
}

/// Extract module names from lib.rs (`pub mod X` and `mod X`).
fn parse_modules(lib_content: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in lib_content.lines() {
        let trimmed = line.trim();
        // Match: pub mod foo; or mod foo;
        let rest = if let Some(r) = trimmed.strip_prefix("pub mod ") {
            Some(r)
        } else {
            trimmed.strip_prefix("mod ")
        };
        if let Some(rest) = rest {
            if let Some(name) = rest.strip_suffix(';') {
                let name = name.trim();
                // Skip test modules
                if name != "tests" {
                    modules.push(name.to_string());
                }
            }
        }
    }
    modules
}

fn classify_role(internal_deps: &[String], is_binary: bool, has_clap: bool) -> &'static str {
    if is_binary && has_clap {
        "cli"
    } else if is_binary {
        "binary"
    } else if internal_deps.is_empty() {
        "foundation"
    } else if internal_deps.len() <= 2 {
        "domain"
    } else {
        "bridge"
    }
}

/// Check if any .rs file in src/ contains #[cfg(test)].
/// Scans up to two levels deep (src/*.rs and src/*/*.rs).
fn has_inline_tests(crate_dir: &Path) -> bool {
    let src_dir = crate_dir.join("src");
    if !src_dir.is_dir() {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(&src_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "rs") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if content.contains("#[cfg(test)]") {
                        return true;
                    }
                }
            } else if path.is_dir() {
                // One level deeper: src/submod/*.rs
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if sub_path.extension().is_some_and(|e| e == "rs") {
                            if let Ok(content) = std::fs::read_to_string(&sub_path) {
                                if content.contains("#[cfg(test)]") {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

/// Compute topological layers from dependency graph.
/// Returns vec of (layer_index, crates_in_layer), ordered by dependency depth.
fn compute_layers(crates: &[CrateInfo]) -> Vec<(usize, Vec<&CrateInfo>)> {
    let names: HashSet<&str> = crates.iter().map(|c| c.name.as_str()).collect();

    // Build in-degree map (only count deps that are in our set)
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for ci in crates {
        in_degree.entry(&ci.name).or_insert(0);
        dependents.entry(&ci.name).or_default();
    }

    for ci in crates {
        for dep in &ci.internal_deps {
            if names.contains(dep.as_str()) {
                *in_degree.entry(&ci.name).or_insert(0) += 1;
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(&ci.name);
            }
        }
    }

    // Kahn's algorithm with layer tracking
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    // Sort for deterministic output
    let mut sorted: Vec<&str> = queue.drain(..).collect();
    sorted.sort();
    queue.extend(sorted);

    let mut layers: Vec<(usize, Vec<&CrateInfo>)> = Vec::new();
    let mut current_layer = Vec::new();
    let mut current_layer_idx = 0;
    let mut next_queue: Vec<&str> = Vec::new();

    // Index for quick lookup
    let crate_map: HashMap<&str, &CrateInfo> =
        crates.iter().map(|c| (c.name.as_str(), c)).collect();

    while !queue.is_empty() {
        while let Some(id) = queue.pop_front() {
            if let Some(ci) = crate_map.get(id) {
                current_layer.push(*ci);
            }
            if let Some(deps) = dependents.get(id) {
                for &dep in deps {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        next_queue.push(dep);
                    }
                }
            }
        }

        if !current_layer.is_empty() {
            // Sort crates within layer alphabetically
            current_layer.sort_by(|a, b| a.name.cmp(&b.name));
            layers.push((current_layer_idx, current_layer));
            current_layer = Vec::new();
            current_layer_idx += 1;
        }

        next_queue.sort();
        queue.extend(next_queue.drain(..));
    }

    layers
}

fn dedup_layer_names(names: &[&str]) -> String {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for name in names {
        if seen.insert(*name) {
            result.push(*name);
        }
    }
    result.join(" → ")
}

// ── Shared helpers ──

struct Phase {
    id: String,
    prompt: String,
    depends_on: Vec<String>,
}

fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
}

fn escape_yaml_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn detect_workspace_members(cwd: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(cwd.join("Cargo.toml")).ok()?;
    // Simple parse: look for [workspace] section with members = [...]
    if !content.contains("[workspace]") {
        return None;
    }
    let members_start = content.find("members")?;
    let bracket_start = content[members_start..].find('[')?;
    let bracket_end = content[members_start + bracket_start..].find(']')?;
    let members_str =
        &content[members_start + bracket_start + 1..members_start + bracket_start + bracket_end];

    let members: Vec<String> = members_str
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim().trim_matches('"').trim_matches('\'');
            if trimmed.is_empty() {
                return None;
            }
            // Extract crate name from path like "crates/edda-cli"
            let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
            // Skip glob patterns
            if name.contains('*') {
                return None;
            }
            Some(name.to_string())
        })
        .collect();

    if members.is_empty() {
        None
    } else {
        Some(members)
    }
}

// ── Templates ──

const TEMPLATE_RUST_CLI: &str = r#"name: rust-cli
purpose: "TODO: describe what this CLI tool does"

phases:
  - id: scaffold
    prompt: |
      Set up the Rust CLI project structure:
      1. Initialize Cargo.toml with clap as a dependency
      2. Create src/main.rs with basic CLI argument parsing
      3. Create src/lib.rs with core module structure
    check:
      - type: file_exists
        path: Cargo.toml
      - type: cmd_succeeds
        cmd: "cargo check"

  - id: core-logic
    prompt: |
      Implement the core business logic in src/lib.rs.
      Keep functions pure and testable.
    depends_on: [scaffold]
    check:
      - type: cmd_succeeds
        cmd: "cargo check"

  - id: tests
    prompt: |
      Add comprehensive tests:
      1. Unit tests in src/lib.rs
      2. Integration tests in tests/
      3. Ensure all tests pass
    depends_on: [core-logic]
    check:
      - type: cmd_succeeds
        cmd: "cargo test"

  - id: docs
    prompt: |
      Add documentation:
      1. README.md with usage instructions
      2. Doc comments on public API
    depends_on: [core-logic]
    check:
      - type: file_exists
        path: README.md
"#;

const TEMPLATE_RUST_LIB: &str = r#"name: rust-lib
purpose: "TODO: describe what this library provides"

phases:
  - id: scaffold
    prompt: |
      Set up the Rust library project:
      1. Initialize Cargo.toml with appropriate metadata
      2. Create src/lib.rs with module structure
      3. Define public API types and traits
    check:
      - type: file_exists
        path: Cargo.toml
      - type: cmd_succeeds
        cmd: "cargo check"

  - id: api
    prompt: |
      Implement the public API.
      Focus on ergonomic, well-documented interfaces.
    depends_on: [scaffold]
    check:
      - type: cmd_succeeds
        cmd: "cargo check"

  - id: tests
    prompt: |
      Add comprehensive tests:
      1. Unit tests alongside implementation
      2. Integration tests in tests/
      3. Doc tests for public API examples
    depends_on: [api]
    check:
      - type: cmd_succeeds
        cmd: "cargo test"

  - id: docs
    prompt: |
      Add documentation:
      1. README.md with usage examples
      2. Doc comments on all public items
      3. CHANGELOG.md
    depends_on: [api]
    check:
      - type: file_exists
        path: README.md
"#;

const TEMPLATE_PYTHON_API: &str = r#"name: python-api
purpose: "TODO: describe what this API does"

phases:
  - id: scaffold
    prompt: |
      Set up Python REST API project:
      1. Create pyproject.toml with fastapi, uvicorn, pytest
      2. Create src/main.py with FastAPI app and health endpoint
      3. Create src/models.py with Pydantic models
    check:
      - type: file_exists
        path: pyproject.toml
      - type: file_contains
        path: src/main.py
        pattern: "FastAPI"

  - id: endpoints
    prompt: |
      Implement API endpoints in src/routes.py.
      Wire the router into src/main.py.
    depends_on: [scaffold]
    check:
      - type: file_exists
        path: src/routes.py

  - id: tests
    prompt: |
      Add API tests in tests/test_api.py.
      Use httpx AsyncClient with FastAPI TestClient.
    depends_on: [endpoints]
    check:
      - type: file_exists
        path: tests/test_api.py

  - id: docs
    prompt: |
      Create README.md with setup instructions,
      API endpoint documentation, and testing guide.
    depends_on: [endpoints]
    check:
      - type: file_exists
        path: README.md
"#;

const TEMPLATE_NODE_APP: &str = r#"name: node-app
purpose: "TODO: describe what this app does"

phases:
  - id: scaffold
    prompt: |
      Set up Node.js project:
      1. Initialize package.json with dependencies
      2. Set up TypeScript configuration
      3. Create src/index.ts entry point
    check:
      - type: file_exists
        path: package.json
      - type: file_exists
        path: src/index.ts

  - id: features
    prompt: |
      Implement main application features.
      Use TypeScript with strict type checking.
    depends_on: [scaffold]
    check:
      - type: cmd_succeeds
        cmd: "npx tsc --noEmit"

  - id: tests
    prompt: |
      Add tests using the project's test framework.
      Cover critical paths and edge cases.
    depends_on: [features]
    check:
      - type: cmd_succeeds
        cmd: "npm test"

  - id: docs
    prompt: |
      Create README.md with setup and usage instructions.
    depends_on: [features]
    check:
      - type: file_exists
        path: README.md
"#;

const TEMPLATE_FULLSTACK: &str = r#"name: fullstack-app
purpose: "TODO: describe the application"

phases:
  - id: db-schema
    prompt: |
      Design and implement the database schema.
      Create migration files and seed data.
    check:
      - type: cmd_succeeds
        cmd: "echo 'verify schema'"

  - id: api
    prompt: |
      Build the backend API endpoints.
      Connect to the database layer.
    depends_on: [db-schema]
    check:
      - type: cmd_succeeds
        cmd: "echo 'verify api'"

  - id: frontend
    prompt: |
      Build the frontend UI.
      Connect to the API endpoints.
    depends_on: [db-schema]
    check:
      - type: cmd_succeeds
        cmd: "echo 'verify frontend'"

  - id: integration
    prompt: |
      Integration testing and Docker setup.
      Ensure all components work together.
    depends_on: [api, frontend]
    check:
      - type: cmd_succeeds
        cmd: "echo 'verify integration'"
"#;

const TEMPLATE_MINIMAL: &str = r#"name: my-project
purpose: "TODO: describe your intent"

phases:
  - id: implement
    prompt: |
      TODO: describe what the agent should build
    check:
      - type: cmd_succeeds
        cmd: "echo 'verify output'"
"#;

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_internal_deps_path_format() {
        let cargo = r#"
[package]
name = "edda-derive"

[dependencies]
edda-core = { path = "../edda-core" }
edda-ledger = { path = "../edda-ledger" }
anyhow.workspace = true
serde.workspace = true
"#;
        let deps = parse_internal_deps(cargo);
        assert_eq!(deps, vec!["edda-core", "edda-ledger"]);
    }

    #[test]
    fn parse_internal_deps_workspace_format() {
        let cargo = r#"
[dependencies]
edda-core.workspace = true
serde.workspace = true
"#;
        let deps = parse_internal_deps(cargo);
        assert_eq!(deps, vec!["edda-core"]);
    }

    #[test]
    fn parse_internal_deps_no_deps_section() {
        let cargo = r#"
[package]
name = "foo"

[dev-dependencies]
edda-core = { path = "../edda-core" }
"#;
        let deps = parse_internal_deps(cargo);
        assert!(deps.is_empty(), "should not pick up dev-dependencies");
    }

    #[test]
    fn parse_modules_pub_and_private() {
        let lib = r#"
pub mod types;
pub mod canon;
mod hash;
mod event;

pub use types::*;
"#;
        let mods = parse_modules(lib);
        assert_eq!(mods, vec!["types", "canon", "hash", "event"]);
    }

    #[test]
    fn parse_modules_empty() {
        let lib = "use std::path::Path;\npub fn foo() {}";
        let mods = parse_modules(lib);
        assert!(mods.is_empty());
    }

    #[test]
    fn classify_role_cases() {
        assert_eq!(classify_role(&[], true, true), "cli");
        assert_eq!(classify_role(&[], true, false), "binary");
        assert_eq!(classify_role(&[], false, false), "foundation");
        assert_eq!(
            classify_role(&["edda-core".into()], false, false),
            "domain"
        );
        assert_eq!(
            classify_role(&["a".into(), "b".into()], false, false),
            "domain"
        );
        assert_eq!(
            classify_role(&["a".into(), "b".into(), "c".into()], false, false),
            "bridge"
        );
    }

    #[test]
    fn compute_layers_linear() {
        let crates = vec![
            CrateInfo {
                name: "c".into(),
                internal_deps: vec!["b".into()],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "domain",
            },
            CrateInfo {
                name: "a".into(),
                internal_deps: vec![],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "foundation",
            },
            CrateInfo {
                name: "b".into(),
                internal_deps: vec!["a".into()],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "domain",
            },
        ];
        let layers = compute_layers(&crates);
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].0, 0);
        assert_eq!(layers[0].1[0].name, "a");
        assert_eq!(layers[1].1[0].name, "b");
        assert_eq!(layers[2].1[0].name, "c");
    }

    #[test]
    fn compute_layers_parallel() {
        let crates = vec![
            CrateInfo {
                name: "x".into(),
                internal_deps: vec![],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "foundation",
            },
            CrateInfo {
                name: "y".into(),
                internal_deps: vec![],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "foundation",
            },
            CrateInfo {
                name: "z".into(),
                internal_deps: vec!["x".into(), "y".into()],
                modules: vec![],
                is_binary: false,
                has_clap: false,
                has_tests: false,
                role: "domain",
            },
        ];
        let layers = compute_layers(&crates);
        assert_eq!(layers.len(), 2);
        // Layer 0: x, y (parallel, sorted)
        assert_eq!(layers[0].1.len(), 2);
        assert_eq!(layers[0].1[0].name, "x");
        assert_eq!(layers[0].1[1].name, "y");
        // Layer 1: z
        assert_eq!(layers[1].1[0].name, "z");
    }

    #[test]
    fn sanitize_id_lowercases() {
        assert_eq!(sanitize_id("MyProject"), "myproject");
        assert_eq!(sanitize_id("foo bar"), "foo-bar");
        assert_eq!(sanitize_id("a/b.c"), "a-b-c");
    }

    #[test]
    fn escape_yaml_str_handles_quotes() {
        assert_eq!(escape_yaml_str(r#"say "hello""#), r#"say \"hello\""#);
    }
}
