#!/usr/bin/env python3
"""Enforce source size, module names, lint inheritance, and crate boundaries."""
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
failures = []
PANICKING = {f'clippy::{lint}' for lint in (
    'restriction', 'unwrap_used', 'expect_used', 'panic', 'unreachable', 'todo', 'unimplemented',
    'indexing_slicing', 'string_slice', 'exit')} | {'unsafe_code'}

def waivers(masked):
    """Denied lints any allow or expect names, however spaced or wrapped in cfg_attr."""
    compact = re.sub(r'\s+', '', masked)
    named = set()
    for waiver in re.finditer(r'\b(?:allow|expect)\(([^()]*)\)', compact):
        named |= set(waiver[1].split(','))
    return PANICKING & named


for probe, expected in [
    ('#[allow(clippy::unwrap_used)]', True),
    ('#[cfg_attr(all(), allow(clippy::unwrap_used))]', True),
    ('#[cfg_attr(not(test), expect(clippy::panic))]', True),
    ('#[allow(clippy :: panic)]', True),
    ('# [allow(clippy::indexing_slicing)]', True),
    ('#[ allow (\n    dead_code,\n    clippy::string_slice,\n)]', True),
    ('#![expect(unsafe_code)]', True),
    ('#[allow(dead_code)]', False),
]:
    assert bool(waivers(probe)) == expected, f'waiver probe misread: {probe!r}'

files = sorted((ROOT / 'crates').rglob('*.rs')) + sorted((ROOT / 'src').rglob('*.rs'))
for path in files:
    source = path.read_text()
    name = path.relative_to(ROOT)
    lines = source.splitlines()
    limit = 99 if path.name == 'lib.rs' else 500
    if len(lines) > limit:
        failures.append(f'{name}: {len(lines)} lines exceeds {limit}')
    if path.name in {'mod.rs', 'utils.rs', 'helpers.rs', 'common.rs'}:
        failures.append(f'{name}: name the module by domain')
    if path.name == 'lib.rs' and not source.startswith('//!'):
        failures.append(f'{name}: missing crate documentation')
    # Rustfmt places members of named public domain types at a consistent indentation.
    masked = re.sub(r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/', '', source, flags=re.S)
    for declaration in re.finditer(r'\b(struct|enum)\s+(\w+)[^{;]*\{', masked):
        depth, members = 1, 0
        for line in masked[declaration.end():].splitlines():
            if depth == 1 and re.match(r'\s*(?:pub(?:\([^)]*\))?\s+)?\w+\s*(?::|,|\(|\{)', line):
                members += 1
            depth += line.count('{') - line.count('}')
            if depth <= 0:
                break
        if members > 30:
            failures.append(f'{name}: {declaration[2]} has {members} members, exceeds 30')
    if re.search(r'\b(?:eprintln|println)!', masked):
        failures.append(f'{name}: use tracing for diagnostics')
    # Panicking code stays denied everywhere; tests are exempted by clippy.toml, not by attributes.
    waived = waivers(masked)
    if waived:
        failures.append(f'{name}: {", ".join(sorted(waived))} may not be allowed')

for manifest in [ROOT / 'Cargo.toml', *sorted((ROOT / 'crates').glob('*/Cargo.toml'))]:
    package = tomllib.loads(manifest.read_text())
    if package.get('lints', {}).get('workspace') is not True:
        failures.append(f'{manifest.relative_to(ROOT)}: inherit workspace lints')
    if manifest.parent.name in {'dpm-model', 'dpm-schedule', 'dpm-engine'}:
        allowed = {'dpm-model', 'dpm-schedule'}
        for dependency in package.get('dependencies', {}):
            if dependency.startswith('dpm-') and dependency not in allowed:
                failures.append(f'{manifest.relative_to(ROOT)}: outward dependency {dependency}')

for path in ROOT.rglob('*'):
    if any(part in {'.git', 'target', '.dagplan', '.dpm'} for part in path.relative_to(ROOT).parts):
        continue
    if path.is_file() and (path.name.lower() == 'agent.md' or path.name.endswith(('.bak', ' 2.rs'))):
        failures.append(f'{path.relative_to(ROOT)}: obsolete instruction or leftover file')

if failures:
    sys.exit('\n'.join(failures))
print(f'PASS: structural guardrails across {len(files)} Rust files')
