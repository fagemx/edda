use chrono::Utc;
use edda_chronicle::{
    get_attention_items, resolve_anchor, save_state, synthesize_recap, Anchor, LastRecap,
    RecapOptions, RecapState, SynthesisInput,
};
use edda_core::decision::extract_decision;
use edda_ledger::Ledger;
use edda_store::{project_dir, project_id, store_root};
use std::path::Path;

pub fn execute(
    repo_root: &Path,
    query: Option<&str>,
    project: Option<&str>,
    week: bool,
    since: Option<&str>,
    all: bool,
    json: bool,
) -> anyhow::Result<()> {
    let opts = RecapOptions {
        query: query.map(|s| s.to_string()),
        project: project.map(|s| s.to_string()),
        week,
        since: since.map(|s| s.to_string()),
        all,
        json,
    };

    let anchor = Anchor::from_options(&opts);
    let edda_root = store_root();
    let project_id_val = project_id(repo_root);
    let _project_root = project_dir(&project_id_val);

    // Resolve anchor
    let resolved = resolve_anchor(&anchor, &edda_root, &opts)?;

    // Load ledger
    let ledger = Ledger::open(repo_root)?;

    // Get attention items
    let attention_items = get_attention_items(&ledger, resolved.project_filter.as_deref())?;

    // Collect events
    let events = ledger.iter_events()?;
    let commits: Vec<String> = events
        .iter()
        .filter(|e| e.event_type == "commit")
        .filter_map(|e| {
            let title = e.payload.get("title").and_then(|v| v.as_str())?;
            Some(format!("{}: {}", e.ts.split('T').next()?, title))
        })
        .collect();

    let decisions: Vec<String> = events
        .iter()
        .filter(|e| e.event_type == "note")
        .filter_map(|e| extract_decision(&e.payload))
        .map(|d| {
            format!(
                "{} = {}{}",
                d.key,
                d.value,
                d.reason.map(|r| format!(" — {}", r)).unwrap_or_default()
            )
        })
        .collect();

    // For now, use simplified session analysis
    let session_types = vec!["Mixed activity".to_string()];
    let key_turns = vec![];

    // Find related content (BM25)
    let related_content = vec![];

    // Build synthesis input
    let input = SynthesisInput {
        anchor_description: format!("{:?}", resolved),
        session_types,
        key_turns,
        related_content,
        attention_items,
        commits,
        decisions,
    };

    // Synthesize with LLM (async)
    let output = tokio::runtime::Runtime::new()?.block_on(synthesize_recap(input))?;

    // Update state
    let state = RecapState {
        last_recap: LastRecap {
            timestamp: Utc::now().to_rfc3339(),
            anchor: format!("{:?}", anchor),
            sessions_covered: resolved.session_ids.clone(),
        },
    };
    save_state(&edda_root, &state)?;

    // Output
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_human(&output);
    }

    Ok(())
}

fn print_human(output: &edda_chronicle::RecapOutput) {
    println!("📋 Recap\n");
    println!("淨結果");
    println!("{}\n", output.net_result);
    println!("需要你");
    println!("{}\n", output.needs_you);
    println!("決策脈絡");
    println!("{}\n", output.decision_context);
    println!("關聯");
    println!("{}\n", output.relations);
}
