---
title: OpenClaw Integration
---

# OpenClaw Integration

Edda supports OpenClaw through a bridge plugin that hooks into the OpenClaw lifecycle.

## Install

```bash
edda bridge openclaw install
```

This installs the Edda plugin globally for OpenClaw.

## Uninstall

```bash
edda bridge openclaw uninstall
```

## Health check

```bash
edda doctor openclaw
```

## Prerequisites

- An initialized `.edda/` workspace (`edda init --no-hooks`, then install OpenClaw bridge separately)
- OpenClaw installed and configured
