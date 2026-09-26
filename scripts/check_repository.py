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

if failures:
    sys.exit('\n'.join(failures))
print(f'PASS: sole self-host example, publication hygiene and documentation links ({len(paths)} files)')
