use std::path::Path;

pub fn add(
    repo_root: &Path,
    id: &str,
    globs: &[String],
    rule: &str,
    source: &str,
) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    std::fs::create_dir_all(&paths.patterns_dir)?;

    let pattern = serde_json::json!({
        "id": id,
        "trigger": {
            "file_glob": globs,
            "keywords": []
        },
        "rule": rule,
        "source": source,
        "metadata": {
            "created_at": now_rfc3339(),
            "hit_count": 0,
            "last_triggered": null,
            "status": "active"
        }
    });

    let path = paths.patterns_dir.join(format!("{id}.json"));
    if path.exists() {
        anyhow::bail!("Pattern '{id}' already exists. Remove it first.");
    }
    let json = serde_json::to_string_pretty(&pattern)?;
    edda_store::write_atomic(&path, json.as_bytes())?;
    println!("Added pattern: {id}");
    println!("  globs: {:?}", globs);
    println!("  rule: {rule}");
    Ok(())
}

pub fn remove(repo_root: &Path, id: &str) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let path = paths.patterns_dir.join(format!("{id}.json"));
    if !path.exists() {
        anyhow::bail!("Pattern '{id}' not found.");
    }
    std::fs::remove_file(&path)?;
    println!("Removed pattern: {id}");
    Ok(())
}

pub fn list(repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let patterns = edda_bridge_claude::pattern::load_patterns(&paths.patterns_dir);
    if patterns.is_empty() {
        println!("(no patterns)");
        return Ok(());
    }
    for pat in &patterns {
        println!(
            "{} [{}] {:?} → {}",
            pat.id, pat.metadata.status, pat.trigger.file_glob, pat.rule
        );
        if pat.metadata.hit_count > 0 {
            println!(
                "  hits: {}, last: {}",
                pat.metadata.hit_count,
                pat.metadata.last_triggered.as_deref().unwrap_or("never")
            );
        }
    }
    Ok(())
}

pub fn test(repo_root: &Path, file_path: &str) -> anyhow::Result<()> {
    let paths = edda_ledger::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("No .edda/ workspace found. Run `edda init` first.");
    }
    let patterns = edda_bridge_claude::pattern::load_patterns(&paths.patterns_dir);
    let matched = edda_bridge_claude::pattern::match_patterns(&patterns, file_path);
    if matched.is_empty() {
        println!("No patterns match: {file_path}");
    } else {
        println!("Matched {} pattern(s) for: {file_path}\n", matched.len());
        for pat in &matched {
            println!("  {} → {}", pat.id, pat.rule);
        }
    }
    Ok(())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}
