#!/usr/bin/env python3
"""Check bundled examples, publication file hygiene and local documentation links."""
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
failures = []
expected_plans = {
    'examples/self-host/dpm-alpha.json',
    'examples/self-host/dpm-alpha.expected.json',
}
plans = {p.relative_to(ROOT).as_posix() for p in (ROOT / 'examples').rglob('*.json')}
if plans != expected_plans:
    failures.append(f'bundled plans differ from self-host inventory: {sorted(plans ^ expected_plans)}')
if (ROOT / 'fixtures').exists():
    failures.append('fixtures/: user-facing examples belong only in examples/self-host/')

# The publication set includes uncommitted additions but excludes ignored build/local data.
listed = subprocess.run(
    ['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'],
    cwd=ROOT, capture_output=True, check=True,
).stdout.decode().split('\0')
paths = [ROOT / name for name in set(listed) if name and (ROOT / name).is_file()]
for path in paths:
    relative = path.relative_to(ROOT)
    if any(part in {'.dagplan', 'target', '__pycache__'} for part in relative.parts):
        failures.append(f'{relative}: local/build data in publication set')
    if '.dpm' in relative.parts and path.name not in {'project.toml', '.gitignore'}:
        failures.append(f'{relative}: runtime project data in publication set')
    if re.search(r'\.sqlite3?(?:-(?:wal|shm|journal))?$', path.name):
        failures.append(f'{relative}: database in publication set')
    if path.name == '.DS_Store' or path.name.startswith('._'):
        failures.append(f'{relative}: OS metadata in publication set')
    if path.name in {'.env', 'credentials.toml'} or path.name.endswith(('.pem', '.key', '.pyc')):
        failures.append(f'{relative}: private/generated file in publication set')
    if path.suffix != '.md':
        continue
    for target in re.findall(r'\[[^\]\n]*\]\(([^)\s]+)\)', path.read_text()):
        link = urlsplit(target)
        if link.scheme or link.netloc or not link.path:
            continue
        destination = (path.parent / unquote(link.path)).resolve()
        if not destination.is_relative_to(ROOT) or not destination.exists():
            failures.append(f'{relative}: invalid local documentation link {target}')

# Probe Git's actual matching rules, including paths that do not exist yet.
ignored = [
    'target/ignore-probe', 'crates/probe/target/ignore-probe',
    '.DS_Store', 'docs/.DS_Store', '._README.md', 'docs/._diagram.png',
    '.dpm/state.sqlite', '.dpm/state.sqlite-wal', '.dpm/state.sqlite-shm',
    '.dpm/state.sqlite-journal', '.dpm/local.toml', '.dpm/credentials.toml',
    'scratch/plan.sqlite', 'scratch/plan.sqlite3-wal',
    '.env', '.env.local', 'scripts/__pycache__/probe.pyc',
]
shareable = [
    '.dpm/project.toml', '.dpm/.gitignore', '.env.example',
    'Cargo.lock', 'README.md', 'examples/self-host/dpm-alpha.json', 'plan.toml',
]
probes = {**dict.fromkeys(ignored, True), **dict.fromkeys(shareable, False)}
matches = subprocess.run(
    ['git', '-c', 'core.excludesFile=/dev/null', 'check-ignore',
     '--no-index', '--non-matching', '--verbose', '--stdin', '-z'],
    cwd=ROOT, input='\0'.join(probes) + '\0', capture_output=True, text=True,
)
if matches.returncode not in (0, 1):
    failures.append(f'cannot check ignore rules: {matches.stderr.strip()}')
else:
    fields = matches.stdout.rstrip('\0').split('\0')
    observed = set()
    for source, _, pattern, name in zip(*(iter(fields),) * 4):
        observed.add(name)
        actual = bool(pattern) and not pattern.startswith('!')
        if actual != probes[name] or (actual and source not in {'.gitignore', '.dpm/.gitignore'}):
            failures.append(f'{name}: expected ignored={probes[name]}, matched {source}:{pattern}')
    if observed != probes.keys():
        failures.append('Git did not return every ignore-rule probe')

if failures:
    sys.exit('\n'.join(failures))
print(f'PASS: self-host inventory, publication hygiene, ignore rules and documentation links ({len(paths)} files)')
