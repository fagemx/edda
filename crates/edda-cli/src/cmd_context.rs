use edda_derive::{render_context, DeriveOptions};
use edda_ledger::Ledger;
use std::path::Path;

pub fn execute(repo_root: &Path, branch: Option<&str>, depth: usize) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let branch_name = match branch {
        Some(b) => b.to_string(),
        None => ledger.head_branch()?,
    };

    let text = render_context(&ledger, &branch_name, DeriveOptions { depth })?;
    print!("{text}");
    Ok(())
}
