---
name: dispatch
description: Dispatch tasks to Karvi with correct runtime, provider, and model configuration
---

# Task Dispatch Skill

You are a Karvi dispatch specialist. Use this skill to dispatch GitHub issues as tasks to Karvi's execution engine.

## Quick Reference

### CLI Dispatch (recommended)

```bash
# Basic — uses server's default runtime and model
npm run go -- <issue-number>

# Specify runtime
npm run go -- <issue> --runtime opencode
npm run go -- <issue> --runtime codex

# Specify runtime + model (overrides all defaults)
npm run go -- <issue> --runtime opencode --model <provider-id>/<model-id>

# Cross-project dispatch
npm run go -- <issue> --repo C:\path\to\other\project

# Multiple issues
npm run go -- 100 101 102

# Skip confirmation
npm run go -- <issue> -y
```

### curl Dispatch

```bash
curl -X POST http://localhost:3461/api/projects \
  -H "Content-Type: application/json" \
  -d '{
    "title": "GH-XXX: task title",
    "tasks": [{
      "id": "GH-XXX",
      "title": "feat(scope): description",
      "assignee": "engineer_lite",
      "runtimeHint": "opencode",
      "modelHint": "provider-id/model-id",
      "description": "Implement GitHub issue #XXX. See https://github.com/owner/repo/issues/XXX"
    }]
  }'
```

## Runtimes

| Runtime | Tool | Best for | Sandbox |
|---------|------|----------|---------|
| `openai-api` | Direct HTTP (no CLI) | Any OpenAI-compatible API (z.ai, T8Star, Ollama) | No sandbox |
| `claude-api` | Direct HTTP (no CLI) | Anthropic API direct | No sandbox |
| `opencode` | opencode CLI | Flexible, supports custom providers | No sandbox |
| `codex` | Codex CLI | OpenAI models, sandboxed execution | workspace-write |
| `claude` | Claude Code CLI | Anthropic models | No sandbox |

### openai-api runtime (recommended for z.ai / T8Star / Ollama)

Direct HTTP call — no CLI needed. Karvi server handles conversation loop + tool execution (read_file, write_file, bash, list_directory, skill). Provider config from `opencode.json`.

```bash
# z.ai via openai-api (direct HTTP, no opencode CLI)
npm run go -- <issue> --runtime openai-api --model zai-coding-plan/glm-5

# T8Star via openai-api
npm run go -- <issue> --runtime openai-api --model custom-ai-t8star-cn/gpt-5.3-codex-medium

# Ollama via openai-api
npm run go -- <issue> --runtime openai-api --model ollama-local/qwen3.5:35b-32k
```

**IMPORTANT**: z.ai GLM Coding Plan requires the dedicated coding endpoint:
- ✅ `https://api.z.ai/api/coding/paas/v4` (coding scenarios)
- ❌ `https://api.z.ai/api/paas/v4` (general — returns 429 for coding)

### Local models (Ollama)

Ollama models go through `opencode` runtime via custom provider in `opencode.json`:

```json
"ollama-local": {
  "name": "Ollama Local",
  "npm": "@ai-sdk/openai-compatible",
  "env": [],
  "models": {
    "qwen3.5:35b-32k": {
      "name": "Qwen 3.5 35B 32K",
      "tool_call": true,
      "limit": { "context": 32768, "output": 8192 }
    }
  },
  "options": {
    "baseURL": "http://localhost:11434/v1",
    "apiKey": "ollama"
  }
}
```

Dispatch with `modelHint: "ollama-local/qwen3.5:35b-32k"`. No need to set `runtimeHint` — step-worker auto-infers `opencode` when modelHint contains `/`.

Prerequisites:
1. `ollama serve` running (default port 11434)
2. Model pulled: `ollama pull qwen3.5:35b-32k`
3. `opencode.json` committed (worktrees need it)
4. Model must support tool calling (qwen3, llama3.1+, mistral-nemo)

### How runtime is selected

```
1. task.runtimeHint (--runtime flag or payload)
2. controls.preferred_runtime (server setting)
3. "openclaw" (default)
```

## Model Selection

### Priority chain

```
1. task.modelHint        ← per-task (--model flag or payload)
2. model_map[runtime][stepType]  ← global, per step type
3. model_map[runtime].default    ← global fallback
4. null                          ← runtime uses its own default config
```

### model_map format

The `modelHint` and `model_map` values use format: `<provider-id>/<model-id>`

### Built-in vs Custom providers (IMPORTANT)

opencode has two kinds of providers:

| Type | `opencode.json` needed? | API key | Examples |
|------|------------------------|---------|----------|
| **Built-in** | No — shipped with opencode | User pre-configured at opencode/system level | `anthropic`, `openrouter`, `zai-coding-plan`, ... |
| **Custom** | Yes — must exist in project root | In karvi `.env` or system env | User-defined entries in `opencode.json` |

Built-in providers and their default models **vary per user's opencode installation and configuration**. Do not assume any specific provider or model is available.

**How to check what's actually configured:**

```bash
# 1. Check current model_map (what Karvi will use)
curl -s http://localhost:3461/api/controls | node -e "
  const d=JSON.parse(require('fs').readFileSync(0,'utf8'));
  console.log('preferred_runtime:', d.preferred_runtime);
  console.log('model_map:', JSON.stringify(d.model_map, null, 2))"

# 2. Check what opencode sees (from Karvi server startup log)
#    Look for lines like: [opencode] Loaded N provider(s), M model(s)

# 3. Verify a specific model works
opencode run --model <provider>/<model> -- "hello"
```

**Do NOT audit `opencode.json` files to verify built-in providers** — they won't appear there.

The provider-id must match either:
- A built-in opencode provider, OR
- A custom provider defined in `opencode.json` in the project root

### Setting global model_map

```bash
curl -X POST http://localhost:3461/api/controls \
  -H "Content-Type: application/json" \
  -d '{"model_map": {"opencode": {"default": "provider/model"}}}'
```

### Clearing model_map (use runtime defaults)

```bash
curl -X POST http://localhost:3461/api/controls \
  -H "Content-Type: application/json" \
  -d '{"model_map": {}}'
```

## Cross-Project Dispatch

To dispatch tasks to a different repo (e.g., edda issues dispatched from karvi):

### target_repo formats (IMPORTANT)

| Format | Example | OK? |
|--------|---------|-----|
| Absolute path | `"C:\\ai_agent\\edda"` or `"C:/ai_agent/edda"` | ✅ |
| GitHub slug + repo_map | `"fagemx/edda"` | ✅ (needs repo_map) |
| Relative path | `"../edda"` | ❌ Rejected |
| Unescaped Windows path | `"C:\ai_agent\edda"` | ❌ Backslash escape bug |

### What changes with cross-project dispatch

| Item | Without `target_repo` | With `target_repo` |
|------|----------------------|-------------------|
| Worktree location | `karvi/.claude/worktrees/` | `{target_repo}/.claude/worktrees/` |
| Skills loaded | karvi's `.claude/skills/` | target repo's `.claude/skills/` |
| Agent CWD | karvi worktree | target repo worktree |
| `opencode.json` | karvi's | **target repo's** (only matters for custom providers) |
| CLAUDE.md | karvi's | target repo's |
| Task tracking | karvi board | karvi board (centralized) |

### Prerequisites for cross-project dispatch

1. Target repo must be a valid Git repo
2. **Built-in providers** (e.g., `zai-coding-plan`): work out of the box, no extra setup needed
3. **Custom providers** (e.g., `custom-ai-t8star-cn`): target repo must have its own `opencode.json` with the provider definition
4. repo_map configured (if using slug format)

### Setup repo_map for slug-based dispatch

```bash
curl -X POST http://localhost:3461/api/controls \
  -H "Content-Type: application/json" \
  -d '{"repo_map": {"fagemx/edda": "C:/ai_agent/edda"}}'
```

### CLI cross-project (simplest)

```bash
# --repo accepts raw Windows path (shell handles escaping)
npm run go -- <issue> --repo C:\path\to\target\repo

# Multiple issues + model override
npm run go -- 100 101 102 --repo C:\path\to\target\repo --runtime opencode --model <provider>/<model> -y
```

### curl cross-project

```bash
curl -X POST http://localhost:3461/api/projects \
  -H "Content-Type: application/json" \
  -d '{
    "title": "EDDA-145: task title",
    "tasks": [{
      "id": "EDDA-145",
      "title": "feat: description",
      "assignee": "engineer_lite",
      "target_repo": "fagemx/edda",
      "description": "Implement issue fagemx/edda#145"
    }]
  }'
```

## Multi-Model Dispatch (different model per task)

Each task can use a different provider/model via `--model`:

```bash
# Each task gets its own model
npm run go -- 100 --runtime opencode --model <provider-A>/<model-A>
npm run go -- 101 --runtime opencode --model <provider-B>/<model-B>

# Without --model → uses model_map default → then opencode's own default
npm run go -- 102 --runtime opencode

# Different runtime entirely
npm run go -- 103 --runtime codex
```

To find available `<provider>/<model>` values, check `controls.model_map` or ask the user which model to use. Do not guess.

## Pre-Dispatch Checklist

1. **Server running?** `curl http://localhost:3461/api/health/preflight`
   - If not: `cd C:/ai_agent/karvi/server && node server.js &`
2. **Controls correct?** `curl http://localhost:3461/api/controls`
   - `use_worktrees: true` — isolated workspaces
   - `use_step_pipeline: true` — plan → implement → review
3. **Model available?** Check `controls.model_map` for configured defaults. If user specifies a model, trust it. Do NOT audit config files for built-in providers — just dispatch.
4. **Cross-project only**: custom providers need `opencode.json` in target repo root

## Common Pitfalls

| Mistake | Fix |
|---------|-----|
| Auditing config files for built-in providers | Built-in providers (e.g., `zai-coding-plan`) don't appear in `opencode.json` — just dispatch |
| `autoStart: true` in payload | Don't use — bypasses worktree + step pipeline |
| Windows path not escaped in JSON | Use `\\` or `/` in JSON strings |
| Custom provider not found in cross-project | Ensure `opencode.json` is in **target repo** root (not just karvi) |
| model_map overrides your intent | Clear it: `{"model_map": {}}` |
| Task stuck as "dispatched" | Manual dispatch: `curl -X POST .../api/tasks/GH-XXX/dispatch` |
| Ollama/local model 走到 openclaw | `modelHint` 含 `/` 會自動推斷 opencode runtime（step-worker inferredRuntime），不需手動設 `runtimeHint` |
| Worktree 缺 `opencode.json` 導致 provider not found | `opencode.json` 未 commit 時 worktree 不會有。先 commit 或手動 copy 到 worktree |
| Ollama provider 需要 `apiKey` 欄位 | Ollama 不驗 key，但 opencode SDK 要求欄位存在，填 `"apiKey": "ollama"` 在 options 裡 |
| z.ai implement 沒建 PR | `GITHUB_TOKEN` 必須在 `.env` 裡且 load-env 能讀到。auto-PR 用 GitHub REST API 建 PR（#592） |
| `GITHUB_TOKEN` 設了但 auto-PR 沒觸發 | load-env 用 `!process.env[key]` — 如果 shell 已有空值不會覆蓋。確認 `.env` 位置在 cwd 或 package root |
| `headBefore is not defined` crash | #582 引入的 scoping bug，已修（let → var）。如果遇到類似 retry-poller 誤判 "spawn failed"，先看 server log 真正的 error |
| 非 GH 任務（custom ID）prompt 有 `gh issue view` | 已修：isGitHubIssue 分流，非 GH 任務用精簡 prompt 不含 gh CLI |
| Agent prompt 裡的 `gh` 命令全部移除 | #595 已完成。所有 GitHub 操作由 Karvi server 透過 REST API 處理，agent 不需要 `gh` CLI |
| z.ai 429 余额不足 | 使用 coding endpoint（`api.z.ai/api/coding/paas/v4`），不是 general endpoint |
| openai-api runtime 比 opencode 更穩 | openai-api 直接 HTTP call，不經過 opencode CLI。推薦用於 z.ai/T8Star/Ollama |
| Ollama provider 需要 `apiKey` 欄位 | Ollama 不驗 key，但 opencode SDK 要求欄位存在，填 `"apiKey": "ollama"` 在 options 裡 |

## Monitoring

```bash
# Quick status
curl -s http://localhost:3461/api/board | node -e "
  const d=JSON.parse(require('fs').readFileSync(0,'utf8'));
  d.taskPlan?.tasks?.forEach(t => {
    console.log(t.id, t.status, t.modelHint||'');
    (t.steps||[]).forEach(s => console.log('  ', s.step_id, s.state));
  })"

# Dashboard
open http://localhost:3461
```
