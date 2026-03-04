use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionType {
    Coding,
    Research,
    Discussion,
    Analysis,
    Automated,
    Debugging,
    QuickOps,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionScore {
    pub session_type: SessionType,
    pub score: f64,
}

pub fn classify_session(
    tool_names: &[String],
    bash_commands: &[String],
    edit_count: usize,
    read_count: usize,
    turn_count: usize,
    duration_secs: u64,
) -> SessionType {
    let scores = calculate_scores(
        tool_names,
        bash_commands,
        edit_count,
        read_count,
        turn_count,
        duration_secs,
    );

    scores
        .into_iter()
        .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
        .map(|s| s.session_type)
        .unwrap_or(SessionType::QuickOps)
}

fn calculate_scores(
    tool_names: &[String],
    bash_commands: &[String],
    edit_count: usize,
    read_count: usize,
    turn_count: usize,
    duration_secs: u64,
) -> Vec<SessionScore> {
    let mut scores = Vec::new();

    let tool_set: std::collections::HashSet<&str> = tool_names.iter().map(|s| s.as_str()).collect();
    let has_edit = edit_count > 0;
    let has_write = tool_set.contains("Write");
    let has_read = read_count > 0;
    let has_bash = !bash_commands.is_empty();
    let has_commit = bash_commands.iter().any(|c| c.contains("git commit"));
    let has_test = bash_commands
        .iter()
        .any(|c| c.contains("test") || c.contains("cargo test"));
    let has_grep = tool_set.contains("Grep") || tool_set.contains("Glob");
    let long_duration = duration_secs > 1800;
    let few_tools = tool_names.len() < 5;
    let few_turns = turn_count < 5;

    // Coding: Edit多, Bash(git commit), Read/Grep多
    let coding_score = if has_edit && (has_commit || has_test) {
        3.0 + edit_count as f64 * 0.1
    } else if has_edit && has_read {
        2.0 + edit_count as f64 * 0.1
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Coding,
        score: coding_score,
    });

    // Research: Read/Grep/Glob多, Write(.md), Edit少
    let research_score = if (has_read || has_grep) && has_write && edit_count < 3 {
        3.0 + read_count as f64 * 0.1
    } else if has_read && has_grep {
        2.0
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Research,
        score: research_score,
    });

    // Discussion: 几乎没有tool, turns多, duration长
    let discussion_score = if few_tools && turn_count > 10 && long_duration {
        3.0 + turn_count as f64 * 0.1
    } else if few_tools && turn_count > 5 {
        2.0
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Discussion,
        score: discussion_score,
    });

    // Analysis: Read+Grep+Write(.md)+Edit(.md)
    let analysis_score = if has_read && has_grep && has_write && has_edit {
        3.5
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Analysis,
        score: analysis_score,
    });

    // Automated: SubagentStart/Stop, Bash多
    let automated_score = if tool_set.contains("SubagentStart") || tool_set.contains("SubagentStop")
    {
        4.0
    } else if has_bash && turn_count > 10 {
        2.0
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Automated,
        score: automated_score,
    });

    // Debugging: Bash(test/cargo)多, failed_commands多 (simplified for now)
    let debugging_score = if has_test && has_edit {
        3.5
    } else if has_test {
        2.5
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::Debugging,
        score: debugging_score,
    });

    // Quick ops: 少tool, 少turn, 有commit
    let quick_ops_score = if few_turns && few_tools && turn_count < 5 {
        3.0
    } else {
        0.0
    };
    scores.push(SessionScore {
        session_type: SessionType::QuickOps,
        score: quick_ops_score,
    });

    scores
}
