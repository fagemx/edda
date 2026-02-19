# Edda Conductor Demo

Live demo of Edda's Conductor orchestrating multiple AI agents building a project.

## What it shows

4 phases → 4 Claude Code agents, working sequentially with dependency tracking:

```
scaffold → endpoints → tests  (sequential chain)
                    → docs   (parallel with tests)
```

Each agent:
- Receives context about what previous agents did
- Gets verification checks to pass before moving on
- Auto-retries if checks fail

## Quick start

### Linux / macOS (tmux)

```bash
./run-demo.sh
# Opens 3-pane tmux: conductor | file tree | edda log
# Press Enter to start
```

### Windows (Windows Terminal)

```powershell
.\run-demo.ps1
# Opens split Windows Terminal
# Run: edda conduct run plan.yaml
```

### Manual

```bash
# 1. Create temp workspace
mkdir /tmp/edda-demo && cd /tmp/edda-demo
cp /path/to/demo/plan.yaml .

# 2. Dry run (see the plan)
edda conduct run plan.yaml --dry-run

# 3. Run for real
edda conduct run plan.yaml
```

## Recording for GIF

```bash
# Install asciinema + agg
pip install asciinema
cargo install --git https://github.com/asciinema/agg

# Record
asciinema rec demo.cast -c './run-demo.sh'

# Convert to GIF
agg demo.cast demo.gif --cols 120 --rows 35 --speed 3
```

## Plan structure

| Phase | Task | Depends on | Checks |
|-------|------|------------|--------|
| scaffold | FastAPI project setup | — | file_exists, file_contains |
| endpoints | CRUD routes | scaffold | file_exists, file_contains |
| tests | API tests | endpoints | file_exists, file_contains |
| docs | README.md | endpoints | file_exists, file_contains |
