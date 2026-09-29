#!/usr/bin/env python3
"""Verify a release archive, then exercise both binaries outside the source checkout."""
import argparse
import hashlib
import json
import os
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath


def unpack(archive, destination):
    checksum = Path(str(archive) + '.sha256').read_text().strip().split('  ')
    assert checksum == [hashlib.sha256(archive.read_bytes()).hexdigest(), archive.name], 'archive checksum mismatch'
    with tarfile.open(archive) as bundle:
        entries = bundle.getmembers()
        roots = {PurePosixPath(entry.name).parts[0] for entry in entries}
        assert len(roots) == 1, 'one archive root required'
        seen = set()
        for entry in entries:
            path = PurePosixPath(entry.name)
            assert entry.isfile() and not path.is_absolute() and '..' not in path.parts, entry.name
            assert entry.name not in seen, 'duplicate archive entry'
            seen.add(entry.name)
            target = destination.joinpath(*path.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bundle.extractfile(entry).read())
            target.chmod(entry.mode & 0o777)
    root = destination / roots.pop()
    manifest = json.loads((root / 'manifest.json').read_text())
    actual = {file.relative_to(root).as_posix() for file in root.rglob('*') if file.is_file()}
    assert actual == set(manifest['sha256']) | {'manifest.json'}
    for name, expected in manifest['sha256'].items():
        assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
    assert manifest['license'] == 'AGPL-3.0-only'
    for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'ci.sh', 'README.md', 'docs/mcp.md']:
        assert (root / 'source' / name).is_file(), f'missing corresponding source: {name}'
    return root, manifest


def exercise(root, manifest, directory):
    home, project = directory / 'home', directory / 'project'
    home.mkdir()
    project.mkdir()
    env = {**os.environ, 'HOME': str(home), 'DPM_CONFIG_DIR': str(home / 'dpm'),
           'XDG_CONFIG_HOME': str(home / 'config')}
    cli, mcp = root / 'bin/dpm', root / 'bin/dpm-mcp'

    def run(*args, binary=cli):
        return subprocess.check_output([str(binary), *args], cwd=project, env=env, timeout=30).decode()

    for binary in [cli, mcp]:
        assert run('--version', binary=binary).strip().endswith(manifest['version'])
    run('init', 'Release qualification', '--json')
    plan = json.loads(run('plan', 'template', '--json'))['data']
    candidate = directory / 'candidate.json'
    candidate.write_text(json.dumps(plan))
    run('plan', 'apply', str(candidate), '--reason', 'Synthetic package qualification',
        '--actor', 'service:package-reviewer', '--json')
    run('ratify', 'TEMPLATE-DESIGN', '--actor', 'service:package-reviewer', '--json')
    run('claim', 'TEMPLATE-DESIGN', '--actor', 'agent:package-worker', '--json')
    run('release', 'TEMPLATE-DESIGN', '--reason', 'Finished package probe',
        '--actor', 'agent:package-worker', '--json')
    status = json.loads(run('status', '--no-simulation', '--json'))
    requests = [
        {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
            'protocolVersion': '2025-11-25', 'capabilities': {},
            'clientInfo': {'name': 'release-qualification', 'version': '1'}}},
        {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
        {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'},
        {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call', 'params': {
            'name': 'project_status', 'arguments': {'probabilistic': False}}},
    ]
    result = subprocess.run([str(mcp), '--actor', 'agent:package-reader'], cwd=project, env=env,
                            input=''.join(json.dumps(r) + '\n' for r in requests),
                            capture_output=True, text=True, check=True, timeout=30)
    responses = [json.loads(line) for line in result.stdout.splitlines()]
    assert [r['id'] for r in responses] == [1, 2, 3], responses
    names = {t['name'] for t in responses[1]['result']['tools']}
    assert {'next_work', 'claim_work', 'plan_schema', 'plan_template'} <= names
    assert responses[2]['result']['structuredContent'] == status
    report = json.loads(run('verify-store', '--json'))
    assert report['revision'] == report['data']['revision'] == 4, report
    backup, restored = directory / 'backup.sqlite', directory / 'restored.sqlite'
    run('backup', '--to', str(backup), '--json')
    run('restore', '--from', str(backup), '--to', str(restored), '--json')
    assert json.loads(run('--database', str(restored), 'export')) == json.loads(run('export'))
    # The restored copy continues the same operations under its own lineage.
    copied, original = (json.loads(run(*database, 'history', '--json')) for database in (('--database', str(restored)), ()))
    assert copied['data']['entries'] == original['data']['entries'], (copied, original)
    assert copied['lineage_id'] != original['lineage_id'], (copied['lineage_id'], original['lineage_id'])
    return env


def verify(archive):
    with tempfile.TemporaryDirectory(prefix='dpm-installed-') as temporary:
        directory = Path(temporary)
        root, manifest = unpack(archive, directory)
        env = exercise(root, manifest, directory)
        env.update(DPM_BIN=str(root / 'bin/dpm'), DPM_MCP_BIN=str(root / 'bin/dpm-mcp'))
        subprocess.run([sys.executable, str(root / 'source/scripts/smoke_terminal.py')],
                       cwd=directory, env=env, check=True, timeout=120)
        corrupt = directory / 'corrupt.tar.gz'
        corrupt.write_bytes(archive.read_bytes()[:-16])
        Path(str(corrupt) + '.sha256').write_text(
            Path(str(archive) + '.sha256').read_text().replace(archive.name, corrupt.name))
        try:
            unpack(corrupt, directory / 'refused')
        except AssertionError as error:
            assert str(error) == 'archive checksum mismatch'
        else:
            raise AssertionError('corrupt archive accepted')
        assert not (directory / 'refused').exists()
    print(f'PASS: {manifest["target"]} package checksums, corresponding source, isolated CLI/MCP/TUI, ownership and history recovery')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('archive', type=Path)
    verify(parser.parse_args().archive.resolve())
