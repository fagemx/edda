//! Decision dependencies, causal chain traversal, and outcome metrics.

use edda_core::types::Provenance;
use rusqlite::params;

use super::mappers::*;
use super::types::*;
use super::SqliteStore;

impl SqliteStore {
    // ── Decision Dependencies ────────────────────────────────────────

    /// Insert a dependency edge.
    pub fn insert_dep(
        &self,
        source_key: &str,
        target_key: &str,
        dep_type: &str,
        created_event: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = time_now_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO decision_deps
             (source_key, target_key, dep_type, created_event, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![source_key, target_key, dep_type, created_event, now],
        )?;
        Ok(())
    }

    /// What does `key` depend on?
    pub fn deps_of(&self, key: &str) -> anyhow::Result<Vec<DepRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, target_key, dep_type, created_event, created_at
             FROM decision_deps WHERE source_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], map_dep_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("deps_of query failed: {e}"))
    }

    /// Who depends on `key`?
    pub fn dependents_of(&self, key: &str) -> anyhow::Result<Vec<DepRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_key, target_key, dep_type, created_event, created_at
             FROM decision_deps WHERE target_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], map_dep_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("dependents_of query failed: {e}"))
    }

    /// Transitive dependents of `key` via BFS, up to `max_depth` hops.
    /// Returns `(DepRow, DecisionRow, depth)` tuples, deduplicated by key
    /// (shortest path wins). Only active decisions are included.
    pub fn transitive_dependents_of(
        &self,
        key: &str,
        max_depth: usize,
    ) -> anyhow::Result<Vec<(DepRow, DecisionRow, usize)>> {
        use std::collections::{HashSet, VecDeque};

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(key.to_string());
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((key.to_string(), 0));

        let mut results: Vec<(DepRow, DecisionRow, usize)> = Vec::new();

        while let Some((current_key, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            let deps = self.active_dependents_of(&current_key)?;
            for (dep, decision) in deps {
                if visited.insert(decision.key.clone()) {
                    let next_depth = depth + 1;
                    queue.push_back((decision.key.clone(), next_depth));
                    results.push((dep, decision, next_depth));
                }
            }
        }

        Ok(results)
    }

    /// Who depends on `key`, joined with active decisions only.
    pub fn active_dependents_of(&self, key: &str) -> anyhow::Result<Vec<(DepRow, DecisionRow)>> {
        let mut stmt = self.conn.prepare(
            "SELECT dd.source_key, dd.target_key, dd.dep_type, dd.created_event, dd.created_at,
                    d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                    d.supersedes_id, d.is_active, e.ts,
                    d.scope, d.source_project_id, d.source_event_id,
                    d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
             FROM decision_deps dd
             JOIN decisions d ON d.key = dd.source_key AND d.is_active = TRUE
             JOIN events e ON d.event_id = e.event_id
             WHERE dd.target_key = ?1",
        )?;
        let rows = stmt.query_map(params![key], |row| {
            let dep = DepRow {
                source_key: row.get(0)?,
                target_key: row.get(1)?,
                dep_type: row.get(2)?,
                created_event: row.get(3)?,
                created_at: row.get(4)?,
            };
            let decision = DecisionRow {
                event_id: row.get(5)?,
                key: row.get(6)?,
                value: row.get(7)?,
                reason: row.get(8)?,
                domain: row.get(9)?,
                branch: row.get(10)?,
                supersedes_id: row.get(11)?,
                is_active: row.get(12)?,
                ts: row.get(13)?,
                scope: row.get(14)?,
                source_project_id: row.get(15)?,
                source_event_id: row.get(16)?,
                status: row.get(17)?,
                authority: row.get(18)?,
                affected_paths: row.get(19)?,
                tags: row.get(20)?,
                review_after: row.get(21)?,
                reversibility: row.get(22)?,
                village_id: row.get(23)?,
            };
            Ok((dep, decision))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("active_dependents_of query failed: {e}"))
    }

    // ── Causal Chain ─────────────────────────────────────────────────

    /// Traverse the causal chain from a root decision via unified BFS.
    pub fn causal_chain(
        &self,
        event_id: &str,
        max_depth: usize,
    ) -> anyhow::Result<Option<(DecisionRow, Vec<ChainEntry>)>> {
        use std::collections::{HashSet, VecDeque};

        let root = match self.get_decision_by_event_id(event_id)? {
            Some(d) => d,
            None => return Ok(None),
        };

        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.event_id.clone());

        let mut queue: VecDeque<(String, String, Option<String>, usize)> = VecDeque::new();
        queue.push_back((
            root.key.clone(),
            root.event_id.clone(),
            root.supersedes_id.clone(),
            0,
        ));

        let mut results: Vec<ChainEntry> = Vec::new();

        while let Some((current_key, current_event_id, current_supersedes_id, depth)) =
            queue.pop_front()
        {
            if depth >= max_depth {
                continue;
            }
            let next_depth = depth + 1;

            // 1) Dependency edges: who depends on current_key
            let dep_stmt_sql = "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts,
                        d.scope, d.source_project_id, d.source_event_id,
                        d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id,
                        dd.dep_type
                 FROM decision_deps dd
                 JOIN decisions d ON d.key = dd.source_key
                 JOIN events e ON d.event_id = e.event_id
                 WHERE dd.target_key = ?1";
            let mut dep_stmt = self.conn.prepare(dep_stmt_sql)?;
            let dep_rows = dep_stmt.query_map(params![current_key], |row| {
                let decision = DecisionRow {
                    event_id: row.get(0)?,
                    key: row.get(1)?,
                    value: row.get(2)?,
                    reason: row.get(3)?,
                    domain: row.get(4)?,
                    branch: row.get(5)?,
                    supersedes_id: row.get(6)?,
                    is_active: row.get(7)?,
                    ts: row.get(8)?,
                    scope: row.get(9)?,
                    source_project_id: row.get(10)?,
                    source_event_id: row.get(11)?,
                    status: row.get(12)?,
                    authority: row.get(13)?,
                    affected_paths: row.get(14)?,
                    tags: row.get(15)?,
                    review_after: row.get(16)?,
                    reversibility: row.get(17)?,
                    village_id: row.get(18)?,
                };
                let dep_type: String = row.get(19)?;
                Ok((decision, dep_type))
            })?;
            for row in dep_rows {
                let (decision, dep_type) =
                    row.map_err(|e| anyhow::anyhow!("causal_chain dep query failed: {e}"))?;
                if visited.insert(decision.event_id.clone()) {
                    let relation = match dep_type.as_str() {
                        "explicit" => "depends_on".to_string(),
                        "auto_domain" => "domain_related".to_string(),
                        other => other.to_string(),
                    };
                    queue.push_back((
                        decision.key.clone(),
                        decision.event_id.clone(),
                        decision.supersedes_id.clone(),
                        next_depth,
                    ));
                    results.push(ChainEntry {
                        decision,
                        relation,
                        depth: next_depth,
                    });
                }
            }

            // 2) Superseded-by: decisions whose supersedes_id = current_event_id
            let mut sup_by_stmt = self.conn.prepare(
                "SELECT d.event_id, d.key, d.value, d.reason, d.domain, d.branch,
                        d.supersedes_id, d.is_active, e.ts,
                        d.scope, d.source_project_id, d.source_event_id,
                        d.status, d.authority, d.affected_paths, d.tags, d.review_after, d.reversibility, d.village_id
                 FROM decisions d
                 JOIN events e ON d.event_id = e.event_id
                 WHERE d.supersedes_id = ?1",
            )?;
            let sup_by_rows = sup_by_stmt.query_map(params![current_event_id], map_decision_row)?;
            for row in sup_by_rows {
                let decision = row
                    .map_err(|e| anyhow::anyhow!("causal_chain superseded_by query failed: {e}"))?;
                if visited.insert(decision.event_id.clone()) {
                    queue.push_back((
                        decision.key.clone(),
                        decision.event_id.clone(),
                        decision.supersedes_id.clone(),
                        next_depth,
                    ));
                    results.push(ChainEntry {
                        decision,
                        relation: "superseded_by".to_string(),
                        depth: next_depth,
                    });
                }
            }

            // 3) Supersedes (reverse): if current has supersedes_id, look up that decision
            if let Some(ref sup_id) = current_supersedes_id {
                if let Some(decision) = self.get_decision_by_event_id(sup_id)? {
                    if visited.insert(decision.event_id.clone()) {
                        queue.push_back((
                            decision.key.clone(),
                            decision.event_id.clone(),
                            decision.supersedes_id.clone(),
                            next_depth,
                        ));
                        results.push(ChainEntry {
                            decision,
                            relation: "supersedes".to_string(),
                            depth: next_depth,
                        });
                    }
                }
            }
        }

        Ok(Some((root, results)))
    }

    // ── Decision Outcomes ─────────────────────────────────────────────

    /// Get aggregated outcome metrics for a decision.
    ///
    /// Queries execution_events that have a `based_on` provenance link to the
    /// given decision event_id, then aggregates success rate, cost, and latency.
    pub fn decision_outcomes(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Option<OutcomeMetrics>> {
        let decision = self.get_event(decision_event_id)?;
        let decision = match decision {
            Some(d) => d,
            None => return Ok(None),
        };

        let dp = edda_core::decision::extract_decision(&decision.payload);
        let (decision_key, decision_value) = match dp {
            Some(d) => (d.key, d.value),
            None => return Ok(None),
        };

        let executions = self.executions_for_decision(decision_event_id)?;

        if executions.is_empty() {
            return Ok(Some(OutcomeMetrics {
                decision_event_id: decision_event_id.to_string(),
                decision_key: decision_key.clone(),
                decision_value: decision_value.clone(),
                decision_ts: decision.ts.clone(),
                total_executions: 0,
                success_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                success_rate: 0.0,
                total_cost_usd: 0.0,
                total_tokens_in: 0,
                total_tokens_out: 0,
                avg_latency_ms: 0.0,
                first_execution_ts: None,
                last_execution_ts: None,
            }));
        }

        let total_executions = executions.len() as u64;
        let success_count = executions.iter().filter(|e| e.status == "success").count() as u64;
        let failed_count = executions.iter().filter(|e| e.status == "failed").count() as u64;
        let cancelled_count = executions
            .iter()
            .filter(|e| e.status == "cancelled")
            .count() as u64;

        let success_rate = if total_executions > 0 {
            (success_count as f64 / total_executions as f64) * 100.0
        } else {
            0.0
        };

        let total_cost_usd: f64 = executions.iter().filter_map(|e| e.cost_usd).sum();
        let total_tokens_in: u64 = executions.iter().filter_map(|e| e.token_in).sum();
        let total_tokens_out: u64 = executions.iter().filter_map(|e| e.token_out).sum();

        let latencies: Vec<u64> = executions.iter().filter_map(|e| e.latency_ms).collect();
        let avg_latency_ms = if !latencies.is_empty() {
            latencies.iter().sum::<u64>() as f64 / latencies.len() as f64
        } else {
            0.0
        };

        let first_execution_ts = executions
            .iter()
            .map(|e| e.ts.as_str())
            .min()
            .map(|s| s.to_string());
        let last_execution_ts = executions
            .iter()
            .map(|e| e.ts.as_str())
            .max()
            .map(|s| s.to_string());

        Ok(Some(OutcomeMetrics {
            decision_event_id: decision_event_id.to_string(),
            decision_key,
            decision_value,
            decision_ts: decision.ts,
            total_executions,
            success_count,
            failed_count,
            cancelled_count,
            success_rate,
            total_cost_usd,
            total_tokens_in,
            total_tokens_out,
            avg_latency_ms,
            first_execution_ts,
            last_execution_ts,
        }))
    }

    /// Get all execution events linked to a decision via `based_on` provenance.
    pub fn executions_for_decision(
        &self,
        decision_event_id: &str,
    ) -> anyhow::Result<Vec<ExecutionLinked>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, ts, payload, refs_provenance FROM events
             WHERE event_type = 'execution_event'
             ORDER BY ts",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut results = Vec::new();
        for row_result in rows {
            let (event_id, ts, payload_str, refs_prov_str) = row_result?;
            let provenance: Vec<Provenance> = match serde_json::from_str(&refs_prov_str) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let has_link = provenance
                .iter()
                .any(|p| p.rel == "based_on" && p.target == decision_event_id);

            if !has_link {
                continue;
            }

            let payload: serde_json::Value = match serde_json::from_str(&payload_str) {
                Ok(p) => p,
                Err(_) => continue,
            };

            let status = payload["result"]["status"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let runtime = payload["runtime"].as_str().map(|s| s.to_string());
            let model = payload["model"].as_str().map(|s| s.to_string());
            let cost_usd = payload["usage"]["cost_usd"].as_f64();
            let token_in = payload["usage"]["token_in"].as_u64();
            let token_out = payload["usage"]["token_out"].as_u64();
            let latency_ms = payload["usage"]["latency_ms"].as_u64();

            results.push(ExecutionLinked {
                event_id,
                ts,
                status,
                runtime,
                model,
                cost_usd,
                token_in,
                token_out,
                latency_ms,
            });
        }

        Ok(results)
    }
}
