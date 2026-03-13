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
        .max_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(val: &str) -> String {
        val.to_string()
    }

    #[test]
    fn test_coding_session() {
        let tools = vec![s("Edit"), s("Read"), s("Bash")];
        let bash = vec![s("git commit -m 'fix'")];
        let result = classify_session(&tools, &bash, 5, 3, 10, 600);
        assert_eq!(result, SessionType::Coding);
    }

    #[test]
    fn test_research_session() {
        let tools = vec![s("Read"), s("Grep"), s("Write")];
        let bash: Vec<String> = vec![];
        let result = classify_session(&tools, &bash, 1, 10, 8, 600);
        assert_eq!(result, SessionType::Research);
    }

    #[test]
    fn test_discussion_session() {
        let tools = vec![s("Read")];
        let bash: Vec<String> = vec![];
        let result = classify_session(&tools, &bash, 0, 0, 15, 3600);
        assert_eq!(result, SessionType::Discussion);
    }

    #[test]
    fn test_analysis_session() {
        // Read + Grep + Write + Edit → Analysis (score 3.5)
        // Avoid test commands to keep Debugging at 0
        let tools = vec![s("Read"), s("Grep"), s("Write"), s("Edit")];
        let bash: Vec<String> = vec![];
        let result = classify_session(&tools, &bash, 2, 5, 8, 600);
        assert_eq!(result, SessionType::Analysis);
    }

    #[test]
    fn test_automated_session() {
        let tools = vec![s("SubagentStart"), s("Read"), s("Bash")];
        let bash = vec![s("some-command")];
        let result = classify_session(&tools, &bash, 0, 1, 5, 300);
        assert_eq!(result, SessionType::Automated);
    }

    #[test]
    fn test_debugging_session() {
        // cargo test + Edit → Debugging (score 3.5)
        // Avoid git commit to keep Coding lower
        let tools = vec![s("Edit"), s("Read"), s("Bash")];
        let bash = vec![s("cargo test")];
        let result = classify_session(&tools, &bash, 1, 2, 8, 600);
        assert_eq!(result, SessionType::Debugging);
    }

    #[test]
    fn test_quick_ops_session() {
        let tools = vec![s("Read")];
        let bash: Vec<String> = vec![];
        let result = classify_session(&tools, &bash, 0, 0, 2, 60);
        assert_eq!(result, SessionType::QuickOps);
    }

    #[test]
    fn test_empty_input_defaults_to_quick_ops() {
        let tools: Vec<String> = vec![];
        let bash: Vec<String> = vec![];
        let result = classify_session(&tools, &bash, 0, 0, 0, 0);
        assert_eq!(result, SessionType::QuickOps);
    }
}
