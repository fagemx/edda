# Deliberately non-conformant control adapter (GH-610 mutation-negative).
# Verbatim copy of the harness's built-in CONTROL_STUB (conformance.py), used
# ONLY for a negative-control harness run with --adapter-cmd, because the
# harness auto-runs its built-in control only in vendor mode. This file is
# never counted as bridge evidence: it exists only to prove the harness
# detects contract violations instead of passing everything.
import sys
try:
    sys.stdin.read()
except Exception:
    pass
sys.stdout.write('{"continue": true}')
