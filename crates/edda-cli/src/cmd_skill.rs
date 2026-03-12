//! `edda skill` subcommand group — skill registry management.

use clap::Subcommand;
use edda_store::skill_registry;
use std::path::Path;

#[derive(Subcommand)]
pub enum SkillCmd {
    /// Scan current project and register/update skills in the registry
    Scan,
    /// List all skills across all projects
    List {
        /// Show only skills for the current project
        #[arg(long)]
        project: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details and version history for a skill
    Show {
        /// Skill name (or skill_id in `name:project` format)
        name: String,
    },
    /// Search skills by name or description
    Search {
        /// Search query (case-insensitive substring match)
        query: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

pub fn execute(cmd: SkillCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        SkillCmd::Scan => execute_scan(repo_root),
        SkillCmd::List { project, json } => execute_list(repo_root, project, json),
        SkillCmd::Show { name } => execute_show(&name),
        SkillCmd::Search { query, json } => execute_search(&query, json),
    }
}

fn execute_scan(repo_root: &Path) -> anyhow::Result<()> {
    let skills = skill_registry::scan_project_skills(repo_root);

    if skills.is_empty() {
        println!("No skills found in {}", repo_root.display());
        return Ok(());
    }

    println!("Found {} skill(s), registering...", skills.len());

    let count = skill_registry::scan_and_register(repo_root)?;
    println!("Registered {count} skill(s) in skill registry.");

    for skill in &skills {
        println!("  {} — {}", skill.name, skill.description);
    }

    Ok(())
}

fn execute_list(repo_root: &Path, project_only: bool, json: bool) -> anyhow::Result<()> {
    let skills = if project_only {
        let pid = edda_store::project_id(repo_root);
        skill_registry::list_skills_by_project(&pid)
    } else {
        skill_registry::list_skills()
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&skills)?);
        return Ok(());
    }

    if skills.is_empty() {
        if project_only {
            println!("No skills registered for this project. Run `edda skill scan` first.");
        } else {
            println!(
                "No skills registered. Run `edda skill scan` in a project to register skills."
            );
        }
        return Ok(());
    }

    println!("Skills ({}):\n", skills.len());
    for s in &skills {
        let versions = s.version_history.len();
        let version_info = if versions > 0 {
            format!(
                " ({} previous version{})",
                versions,
                if versions == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        println!(
            "  {} [{}]{}\n    {}\n    hash: {}\n    last seen: {}\n",
            s.name,
            s.project_name,
            version_info,
            s.description,
            &s.content_hash[..16],
            s.last_seen
        );
    }

    Ok(())
}

fn execute_show(name: &str) -> anyhow::Result<()> {
    // Try exact skill_id first (name:project format)
    if let Some(entry) = skill_registry::get_skill(name) {
        print_skill_detail(&entry);
        return Ok(());
    }

    // Try matching by name across all projects
    let all = skill_registry::list_skills();
    let matches: Vec<_> = all.iter().filter(|s| s.name == name).collect();

    match matches.len() {
        0 => {
            println!("Skill '{name}' not found. Use `edda skill list` to see registered skills.");
        }
        1 => {
            print_skill_detail(matches[0]);
        }
        _ => {
            println!("Multiple skills named '{name}':\n");
            for s in &matches {
                println!("  {} (project: {})", s.skill_id, s.project_name);
            }
            println!("\nUse `edda skill show <name>:<project>` to disambiguate.");
        }
    }

    Ok(())
}

fn print_skill_detail(entry: &skill_registry::SkillEntry) {
    println!("Skill: {}", entry.name);
    println!("  ID:          {}", entry.skill_id);
    println!(
        "  Project:     {} ({})",
        entry.project_name, entry.project_id
    );
    println!("  Path:        {}", entry.relative_path);
    println!("  Description: {}", entry.description);
    println!("  Hash:        {}", entry.content_hash);
    println!("  Registered:  {}", entry.registered_at);
    println!("  Last seen:   {}", entry.last_seen);

    if entry.version_history.is_empty() {
        println!("  Versions:    1 (current)");
    } else {
        println!(
            "  Versions:    {} ({} previous)\n",
            entry.version_history.len() + 1,
            entry.version_history.len()
        );
        println!("  Version history:");
        for (i, v) in entry.version_history.iter().enumerate().rev() {
            println!(
                "    v{}: {} (seen {})",
                i + 1,
                &v.content_hash[..16.min(v.content_hash.len())],
                v.seen_at
            );
        }
        println!(
            "    v{}: {} (current)",
            entry.version_history.len() + 1,
            &entry.content_hash[..16.min(entry.content_hash.len())]
        );
    }
}

fn execute_search(query: &str, json: bool) -> anyhow::Result<()> {
    let query_lower = query.to_lowercase();
    let all = skill_registry::list_skills();
    let matches: Vec<_> = all
        .into_iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&query_lower)
                || s.description.to_lowercase().contains(&query_lower)
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&matches)?);
        return Ok(());
    }

    if matches.is_empty() {
        println!("No skills matching '{query}'.");
        return Ok(());
    }

    println!("Skills matching '{}' ({}):\n", query, matches.len());
    for s in &matches {
        println!("  {} [{}]\n    {}\n", s.name, s.project_name, s.description);
    }

    Ok(())
}
