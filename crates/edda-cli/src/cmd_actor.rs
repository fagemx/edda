use clap::Subcommand;
use edda_core::policy::{load_actors_from_dir, save_actors_to_dir, ActorDef, ActorKind, ActorsConfig};
use std::path::Path;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum ActorCmd {
    /// Add an actor to the project
    Add {
        /// Actor name (alphanumeric, hyphens, underscores)
        name: String,
        /// Role to assign (repeatable)
        #[arg(long = "role")]
        roles: Vec<String>,
        /// Actor kind: user or agent
        #[arg(long, default_value = "user")]
        kind: ActorKind,
        /// Email address (optional)
        #[arg(long)]
        email: Option<String>,
        /// Display name (optional)
        #[arg(long)]
        display_name: Option<String>,
        /// Runtime platform for agents (e.g. "claude", "opencode")
        #[arg(long)]
        runtime: Option<String>,
    },
    /// Remove an actor from the project
    Remove {
        /// Actor name
        name: String,
    },
    /// List all project actors
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Filter by role
        #[arg(long)]
        role: Option<String>,
    },
    /// Show details of a single actor
    Show {
        /// Actor name
        name: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Grant a role to an existing actor
    Grant {
        /// Actor name
        name: String,
        /// Role to add
        #[arg(long)]
        role: String,
    },
    /// Revoke a role from an existing actor
    Revoke {
        /// Actor name
        name: String,
        /// Role to remove
        #[arg(long)]
        role: String,
    },
}

// ── Dispatch ──

pub fn run(cmd: ActorCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        ActorCmd::Add {
            name,
            roles,
            kind,
            email,
            display_name,
            runtime,
        } => add(
            repo_root,
            &name,
            &roles,
            kind,
            email,
            display_name,
            runtime,
        ),
        ActorCmd::Remove { name } => remove(repo_root, &name),
        ActorCmd::List { json, role } => list(repo_root, json, role.as_deref()),
        ActorCmd::Show { name, json } => show(repo_root, &name, json),
        ActorCmd::Grant { name, role } => grant(repo_root, &name, &role),
        ActorCmd::Revoke { name, role } => revoke(repo_root, &name, &role),
    }
}

// ── Helpers ──

fn validate_actor_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("Actor name cannot be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Actor name must contain only alphanumeric characters, hyphens, or underscores: {name}"
        );
    }
    Ok(())
}

fn load_and_check(
    repo_root: &Path,
) -> anyhow::Result<(edda_ledger::paths::EddaPaths, ActorsConfig)> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let cfg = load_actors_from_dir(&paths.edda_dir)?;
    Ok((paths, cfg))
}

// ── Commands ──

fn add(
    repo_root: &Path,
    name: &str,
    roles: &[String],
    kind: ActorKind,
    email: Option<String>,
    display_name: Option<String>,
    runtime: Option<String>,
) -> anyhow::Result<()> {
    validate_actor_name(name)?;

    let (paths, mut cfg) = load_and_check(repo_root)?;

    if cfg.actors.contains_key(name) {
        anyhow::bail!(
            "Actor '{name}' already exists. Remove it first or use grant/revoke to modify roles."
        );
    }

    println!("Added actor: {name}");
    println!("  kind: {kind}");
    if !roles.is_empty() {
        println!("  roles: {}", roles.join(", "));
    }

    let actor = ActorDef {
        roles: roles.to_vec(),
        kind,
        email,
        display_name,
        runtime,
    };

    cfg.actors.insert(name.to_string(), actor);
    cfg.version = 2;
    save_actors_to_dir(&paths.edda_dir, &cfg)?;

    Ok(())
}

fn remove(repo_root: &Path, name: &str) -> anyhow::Result<()> {
    let (paths, mut cfg) = load_and_check(repo_root)?;

    if cfg.actors.remove(name).is_none() {
        anyhow::bail!("Actor '{name}' not found.");
    }

    save_actors_to_dir(&paths.edda_dir, &cfg)?;
    println!("Removed actor: {name}");
    Ok(())
}

fn list(repo_root: &Path, json: bool, role_filter: Option<&str>) -> anyhow::Result<()> {
    let (_paths, cfg) = load_and_check(repo_root)?;

    let actors: Vec<_> = cfg
        .actors
        .iter()
        .filter(|(_, def)| {
            role_filter
                .map(|r| def.roles.contains(&r.to_string()))
                .unwrap_or(true)
        })
        .collect();

    if json {
        for (name, def) in &actors {
            let obj = serde_json::json!({
                "name": name,
                "kind": def.kind,
                "roles": def.roles,
                "email": def.email,
                "display_name": def.display_name,
                "runtime": def.runtime,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else if actors.is_empty() {
        println!("(no actors)");
    } else {
        for (name, def) in &actors {
            println!(
                "{} [{}] roles={}",
                name,
                def.kind,
                if def.roles.is_empty() {
                    "(none)".to_string()
                } else {
                    def.roles.join(", ")
                }
            );
        }
    }
    Ok(())
}

fn show(repo_root: &Path, name: &str, json: bool) -> anyhow::Result<()> {
    let (_paths, cfg) = load_and_check(repo_root)?;

    let def = cfg
        .actors
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Actor '{name}' not found."))?;

    if json {
        let obj = serde_json::json!({
            "name": name,
            "kind": def.kind,
            "roles": def.roles,
            "email": def.email,
            "display_name": def.display_name,
            "runtime": def.runtime,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("Actor: {name}");
        println!("  kind: {}", def.kind);
        println!(
            "  roles: {}",
            if def.roles.is_empty() {
                "(none)".to_string()
            } else {
                def.roles.join(", ")
            }
        );
        if let Some(email) = &def.email {
            println!("  email: {email}");
        }
        if let Some(dn) = &def.display_name {
            println!("  display_name: {dn}");
        }
        if let Some(rt) = &def.runtime {
            println!("  runtime: {rt}");
        }
    }
    Ok(())
}

fn grant(repo_root: &Path, name: &str, role: &str) -> anyhow::Result<()> {
    let (paths, mut cfg) = load_and_check(repo_root)?;

    let def = cfg
        .actors
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("Actor '{name}' not found."))?;

    if def.roles.contains(&role.to_string()) {
        println!("Actor '{name}' already has role '{role}'.");
        return Ok(());
    }

    def.roles.push(role.to_string());
    save_actors_to_dir(&paths.edda_dir, &cfg)?;
    println!("Granted role '{role}' to actor '{name}'.");
    Ok(())
}

fn revoke(repo_root: &Path, name: &str, role: &str) -> anyhow::Result<()> {
    let (paths, mut cfg) = load_and_check(repo_root)?;

    let def = cfg
        .actors
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("Actor '{name}' not found."))?;

    let before_len = def.roles.len();
    def.roles.retain(|r| r != role);
    if def.roles.len() == before_len {
        anyhow::bail!("Actor '{name}' does not have role '{role}'.");
    }

    save_actors_to_dir(&paths.edda_dir, &cfg)?;
    println!("Revoked role '{role}' from actor '{name}'.");
    Ok(())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn setup_workspace() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_actor_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Minimal init: create .edda/ dir with actors.yaml
        let edda_dir = tmp.join(".edda");
        std::fs::create_dir_all(&edda_dir).unwrap();
        std::fs::write(edda_dir.join("actors.yaml"), "version: 2\nactors: {}\n").unwrap();
        // Also need schema.json and HEAD for EddaPaths::is_initialized()
        std::fs::write(edda_dir.join("schema.json"), r#"{"version":4}"#).unwrap();
        std::fs::write(edda_dir.join("HEAD"), "main").unwrap();
        tmp
    }

    #[test]
    fn test_actor_add_and_list() {
        let tmp = setup_workspace();
        add(
            &tmp,
            "alice",
            &["lead".into(), "reviewer".into()],
            ActorKind::User,
            Some("a@b.com".into()),
            None,
            None,
        )
        .unwrap();

        let (_, cfg) = load_and_check(&tmp).unwrap();
        assert!(cfg.actors.contains_key("alice"));
        let alice = cfg.actors.get("alice").unwrap();
        assert_eq!(alice.roles, vec!["lead", "reviewer"]);
        assert_eq!(alice.kind, ActorKind::User);
        assert_eq!(alice.email.as_deref(), Some("a@b.com"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_remove() {
        let tmp = setup_workspace();
        add(
            &tmp,
            "bob",
            &["operator".into()],
            ActorKind::Agent,
            None,
            None,
            Some("claude".into()),
        )
        .unwrap();

        let (_, cfg) = load_and_check(&tmp).unwrap();
        assert!(cfg.actors.contains_key("bob"));

        remove(&tmp, "bob").unwrap();
        let (_, cfg) = load_and_check(&tmp).unwrap();
        assert!(!cfg.actors.contains_key("bob"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_grant_revoke() {
        let tmp = setup_workspace();
        add(
            &tmp,
            "carol",
            &["reviewer".into()],
            ActorKind::User,
            None,
            None,
            None,
        )
        .unwrap();

        grant(&tmp, "carol", "lead").unwrap();
        let (_, cfg) = load_and_check(&tmp).unwrap();
        let carol = cfg.actors.get("carol").unwrap();
        assert!(carol.roles.contains(&"lead".to_string()));
        assert!(carol.roles.contains(&"reviewer".to_string()));

        revoke(&tmp, "carol", "reviewer").unwrap();
        let (_, cfg) = load_and_check(&tmp).unwrap();
        let carol = cfg.actors.get("carol").unwrap();
        assert!(!carol.roles.contains(&"reviewer".to_string()));
        assert!(carol.roles.contains(&"lead".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_add_duplicate_errors() {
        let tmp = setup_workspace();
        add(&tmp, "dave", &[], ActorKind::User, None, None, None).unwrap();
        let result = add(&tmp, "dave", &[], ActorKind::User, None, None, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_remove_nonexistent_errors() {
        let tmp = setup_workspace();
        let result = remove(&tmp, "nobody");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_kind_default() {
        let tmp = setup_workspace();
        // When kind defaults to "user"
        add(&tmp, "eve", &[], ActorKind::User, None, None, None).unwrap();
        let (_, cfg) = load_and_check(&tmp).unwrap();
        let eve = cfg.actors.get("eve").unwrap();
        assert_eq!(eve.kind, ActorKind::User);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_actor_name_validation() {
        assert!(validate_actor_name("alice").is_ok());
        assert!(validate_actor_name("claude-agent-1").is_ok());
        assert!(validate_actor_name("my_bot").is_ok());
        assert!(validate_actor_name("").is_err());
        assert!(validate_actor_name("a b").is_err());
        assert!(validate_actor_name("a@b").is_err());
    }

    #[test]
    fn test_actor_invalid_kind_errors() {
        // ActorKind::from_str rejects invalid values at parse time
        let result: Result<ActorKind, _> = "robot".parse();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be 'user' or 'agent'"));
    }
}
