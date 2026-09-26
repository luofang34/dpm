#!/usr/bin/env python3
"""Validate effective workspace licensing and the scope of cargo-deny exceptions."""
import json
from pathlib import Path
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]
metadata = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--format-version', '1', '--locked', '--all-features'],
    cwd=ROOT, text=True,
))
members = set(metadata['workspace_members'])
workspace = [p for p in metadata['packages'] if p['id'] in members]
policy = tomllib.loads((ROOT / 'deny.toml').read_text())['licenses']
exceptions = policy['exceptions']
expected = {p['name'] for p in workspace}
assert {e['crate'] for e in exceptions} == expected and len(exceptions) == len(expected), 'license exceptions must match workspace membership'
assert all(e['allow'] == ['AGPL-3.0-only'] for e in exceptions), 'workspace exceptions must only allow AGPL-3.0-only'
assert 'AGPL-3.0-only' not in policy['allow'], 'AGPL must not be globally allowed for dependencies'
for package in metadata['packages']:
    if package['id'] in members:
        assert package['source'] is None and package['license'] == 'AGPL-3.0-only', f"{package['name']}: wrong effective workspace license"
    else:
        assert package['name'] not in expected, f"{package['name']}: third-party crate would inherit a workspace exception"
print('PASS: effective workspace licenses and dependency exception boundaries')
