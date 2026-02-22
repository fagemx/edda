---
title: MCP Integration
---

# MCP Integration

Edda provides an MCP server (stdio transport, JSON-RPC 2.0) that works with any MCP-compatible client — Cursor, Windsurf, and others.

## Start the server

```bash
edda mcp serve
```

## Available tools

The MCP server exposes 7 tools:

| Tool | Description |
|------|-------------|
| `edda_status` | Show workspace status |
| `edda_note` | Record a note event |
| `edda_decide` | Record a binding decision |
| `edda_ask` | Query past decisions and history |
| `edda_log` | Query events with filters |
| `edda_context` | Output context snapshot |
| `edda_draft_inbox` | Show pending approval items |

## Client configuration

Add Edda to your MCP client's server configuration. Example for a generic MCP client:

```json
{
  "mcpServers": {
    "edda": {
      "command": "edda",
      "args": ["mcp", "serve"],
      "transport": "stdio"
    }
  }
}
```

The server reads from stdin and writes to stdout using JSON-RPC 2.0.

## Prerequisites

- An initialized `.edda/` workspace (`edda init`)
- The `edda` binary in your PATH
