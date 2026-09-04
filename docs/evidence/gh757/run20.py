"""Capture GH757's required consecutive default-parallel bridge test runs."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

REPO = Path('C:/ai_agent/edda-wt-gh757')
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent / 'evidence-757'
LANE = 'C:/ai_agent/fleet-workstation/lanes/worker-1'


def git(*args):
    return subprocess.check_output(['git', *args], cwd=REPO, text=True).strip()


def snapshot(root):
    return {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob('*')) if p.is_file()}


def main():
    OUT.mkdir(exist_ok=True)
    if (OUT / 'manifest.json').exists():
        raise SystemExit('Existing manifest: preserve evidence; do not overwrite a prior run.')
    if git('status', '--porcelain'):
        raise SystemExit('Freeze source and clean tree before acceptance.')
    env = os.environ.copy()
    env.pop('RUST_TEST_THREADS', None)
    env['CARGO_TARGET_DIR'] = LANE
    sha = git('rev-parse', 'HEAD')
    receipt = {'source_sha': sha, 'command': ['cargo', 'test', '-p', 'edda-bridge-claude'],
               'parallelism': 'default; RUST_TEST_THREADS unset; no test-threads argument',
               'lane': LANE, 'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
               'runs': [], 'complete': False}
    with tempfile.TemporaryDirectory(prefix='gh757-fallback-') as scratch:
        fallback = Path(scratch)
        (fallback / 'sentinel.txt').write_bytes(b'GH757 fallback must remain untouched\n')
        before = snapshot(fallback)
        env['EDDA_STORE_ROOT'] = scratch
        receipt['fallback_before'] = before
        for number in range(1, 21):
            start = time.time()
            stem = f'run-{number:02}'
            stdout = OUT / (stem + '.stdout.log')
            stderr = OUT / (stem + '.stderr.log')
            with stdout.open('wb') as out, stderr.open('wb') as err:
                result = subprocess.run(receipt['command'], cwd=REPO, env=env,
                                        stdout=out, stderr=err)
            after = snapshot(fallback)
            summaries = [line for line in stdout.read_text(encoding='utf-8', errors='replace').splitlines()
                         if line.startswith('test result:')]
            row = {'run': number, 'exit_code': result.returncode,
                   'elapsed_seconds': round(time.time() - start, 3),
                   'summaries': summaries, 'fallback_unchanged': after == before,
                   'fallback_after': after,
                   'stdout_sha256': hashlib.sha256(stdout.read_bytes()).hexdigest(),
                   'stderr_sha256': hashlib.sha256(stderr.read_bytes()).hexdigest()}
            receipt['runs'].append(row)
            receipt['complete'] = (number == 20 and result.returncode == 0 and after == before)
            (OUT / 'manifest.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
            print(json.dumps(row), flush=True)
            if result.returncode or after != before:
                raise SystemExit('Acceptance failed: retain all logs; no automatic retries.')
        if git('rev-parse', 'HEAD') != sha or git('status', '--porcelain'):
            receipt['complete'] = False
            (OUT / 'manifest.json').write_text(json.dumps(receipt, indent=2) + '\n', encoding='utf-8')
            raise SystemExit('Source changed during acceptance.')


if __name__ == '__main__':
    main()
