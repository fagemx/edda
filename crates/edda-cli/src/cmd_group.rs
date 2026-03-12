//! `edda group` — manage project groups for cross-project sync.

use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum GroupCmd {
    /// Assign the current project to a group
    Set {
        /// Group name
        name: String,
    },
    /// Show current project's group and group members
    Show,
    /// List all groups and their projects
    List,
    /// Remove the current project from its group
    Remove,
}

pub fn execute(cmd: GroupCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        GroupCmd::Set { name } => {
            edda_store::registry::register_project(repo_root)?;
            edda_store::registry::set_project_group(repo_root, Some(&name))?;
            println!("Project assigned to group: {name}");
            Ok(())
        }
        GroupCmd::Show => {
            let group = edda_store::registry::project_group(repo_root);
            match group {
                Some(g) => {
                    println!("Group: {g}");
                    let members = edda_store::registry::list_group_members(repo_root);
                    if members.is_empty() {
                        println!("  (no other members)");
                    } else {
                        println!("Members:");
                        for m in &members {
                            println!("  {} ({})", m.name, m.path);
                        }
                    }
                }
                None => {
                    println!("This project is not assigned to any group.");
                    println!("Use `edda group set <name>` to assign it.");
                }
            }
            Ok(())
        }
        GroupCmd::List => {
            let groups = edda_store::registry::list_groups();
            if groups.is_empty() {
                println!("No groups defined.");
                println!("Use `edda group set <name>` in a project to create one.");
            } else {
                for (name, members) in &groups {
                    println!("{name}:");
                    for m in members {
                        println!("  {} ({})", m.name, m.path);
                    }
                }
            }
            Ok(())
        }
        GroupCmd::Remove => {
            edda_store::registry::set_project_group(repo_root, None)?;
            println!("Project removed from group.");
            Ok(())
        }
    }
}
