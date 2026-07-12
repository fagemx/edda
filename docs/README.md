# Edda Documentation

A map of what lives where. Start at the repo [README](../README.md) for the
pitch and install; come here when you need depth.

## For users

| Section | What it answers |
| - | - |
| [`getting-started/`](./getting-started/) | Install ([installation.md](./getting-started/installation.md)) and first five minutes ([quickstart.md](./getting-started/quickstart.md)) |
| [`guides/`](./guides/) | Using Edda with [Claude Code](./guides/claude-code.md), [MCP](./guides/mcp.md), [multi-agent setups](./guides/multi-agent.md), and [OpenClaw](./guides/openclaw.md) |
| [`reference/`](./reference/) | [CLI reference](./reference/cli.md), [brief schema](./reference/brief-schema.md), [query performance](./reference/query-performance.md) |
| [`README_zh-TW.md`](./README_zh-TW.md) | 繁體中文版 README |

## For contributors and the curious

| Section | What it answers |
| - | - |
| [`architecture/`](./architecture/) | How the system holds together: [overview](./architecture/overview.md), [state consistency contract](./architecture/consistency-contract.md) |
| [`decision/`](./decision/) | The decision-subsystem spec stack (v0 design specs): [model](./decision/decision-model/overview.md), [intake](./decision/decision-intake/overview.md), [injection](./decision/decision-injection/overview.md), [governance](./decision/decision-governance/overview.md). The code is authoritative where details differ. |
| [`blog/`](./blog/) | Announcement and design-story posts |

## Plans and history

| Section | What it is |
| - | - |
| [`plan/`](./plan/) | Active plans not yet executed |
| [`archive/plans/`](./archive/plans/) | Executed planning packs, kept as construction history (e.g. [decision-deepening](./archive/plans/decision-deepening/00_OVERVIEW.md)) |
| [`archive/design-notes/`](./archive/design-notes/) | Raw design-conversation notes from the planning phase — provenance, not documentation |

House rule: user-facing docs stay accurate or get fixed; anything under
`archive/` is frozen history and is never updated to match later reality.
