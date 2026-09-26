#!/usr/bin/env python3
"""Exercise the release CLI against a disposable local database."""
import argparse
import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def smoke(binary, directory):
    database = directory / 'smoke.sqlite'
    checks = []

    def run(*args, error=None, structured=False):
        command = [str(binary), '--database', str(database), *args]
        if structured:
            command.append('--json')
        result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
        if error:
            assert result.returncode != 0, (args, result.stdout)
            message = json.loads(result.stdout)['error']['message'] if structured else result.stderr
            assert error in message, (args, message)
        else:
            assert result.returncode == 0, (args, result.stderr)
        checks.append({'command': list(args), 'expected_rejection': bool(error)})
        return json.loads(result.stdout) if structured else result.stdout

    def ready():
        return {item['work']['key'] for item in run('next', structured=True)['candidates']}

    def finish(key):
        run('claim', key, '--actor', 'agent:smoke')
        run('submit', key, '--actor', 'agent:smoke', '--note', 'Smoke acceptance evidence')
        run('verify', key, '--actor', 'human:reviewer')

    assert run('import', str(ROOT / 'tests/support/execution-plan.json'), structured=True)['revision'] == 0
    assert ready() == {'TEST-A'}
    assert run('next', '--capability', 'unrelated', structured=True)['candidates'] == []
    limited = run('next', '--limit', '0', structured=True)
    assert limited['candidates'] == [] and limited['in_scope_count'] == limited['eligible_count'] == 1
    initial = run('status', structured=True)
    assert initial['revision'] == 0 and initial['ready'] == 1
    assert initial['p50_finish_hours'] <= initial['p80_finish_hours'] <= initial['p95_finish_hours']
    run('demo', error='workspace already exists')
    run('claim', 'TEST-B', '--actor', 'agent:smoke', error='not ready')
    run('claim', 'TEST-A', '--actor', 'agent:', error='actor name must not be empty', structured=True)
    run('show', 'UNKNOWN', error='unknown work key')
    assert run('status', structured=True)['revision'] == 0
    assert run('claim', 'TEST-A', '--actor', 'agent:smoke', structured=True)['resulting_revision'] == 1
    run('claim', 'TEST-A', '--actor', 'agent:other', error='not ready')
    run('submit', 'TEST-A', '--actor', 'agent:other', error='owned by')
    run('attach-git-head', 'TEST-A', '--resource', 'TEST-REPO', '--actor', 'agent:smoke')
    assert len(run('show', 'TEST-A', structured=True)['artifact_ids']) == 1
    run('submit', 'TEST-A', '--actor', 'agent:smoke')
    revision = run('status', structured=True)['revision']
    run('verify', 'TEST-A', '--actor', 'agent:smoke', error='cannot verify')
    assert run('status', structured=True)['revision'] == revision
    run('verify', 'TEST-A', '--actor', 'human:reviewer')
    assert ready() == set()
    explanation = run('explain', 'TEST-B', structured=True)
    assert not explanation['ready']
    assert any('TEST-GATE' in reason for reason in explanation['why_now'])
    run('decide', 'TEST-GATE', 'Accept the verified input', '--actor', 'human:reviewer')
    assert ready() == {'TEST-B', 'TEST-D'}
    run('block', 'TEST-B', 'Waiting for review', '--actor', 'human:reviewer')
    assert ready() == {'TEST-D'}
    run('unblock', 'TEST-B', '--actor', 'human:reviewer')
    assert ready() == {'TEST-B', 'TEST-D'}
    for key in ('TEST-B', 'TEST-C', 'TEST-D', 'TEST-E', 'TEST-F'):
        finish(key)
    assert ready() == set()
    final = run('status', structured=True)
    assert final['complete'] == 7 and final['total_work'] == 7
    assert final['expected_finish_hours'] == 0
    assert final['p50_finish_hours'] == final['p80_finish_hours'] == final['p95_finish_hours'] == 0
    assert run('show', 'TEST-M1', structured=True)['status'] == 'Verified'
    run('claim', 'TEST-M1', '--actor', 'agent:smoke', error='not a task')
    with sqlite3.connect(database) as connection:
        count = connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0]
        assert count == final['revision'] == 22
        snapshot = json.loads(connection.execute('SELECT plan_json FROM plan_state').fetchone()[0])
        assert snapshot['revision'] == count
        artifact = next(iter(snapshot['artifacts'].values()))
        head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        assert artifact['metadata']['commit'] == head
    exported = run('export', structured=True)
    assert next(w for w in exported['work_items'].values() if w['key'] == 'TEST-M1')['status'] == 'Planned'
    export_path = directory / 'export.json'
    export_path.write_text(json.dumps(exported))
    assert run('validate', str(export_path), structured=True)['valid']
    database = directory / 'imported.sqlite'
    assert run('import', str(export_path), structured=True)['revision'] == 22
    assert run('export', structured=True) == exported
    run('import', str(export_path), error='workspace already exists', structured=True)
    database = directory / 'invalid.sqlite'
    exported['workspace']['name'] = ''
    export_path.write_text(json.dumps(exported))
    run('import', str(export_path), error='must not be empty', structured=True)
    assert not database.exists()
    database = directory / 'missing.sqlite'
    run('status', error='open', structured=True)
    assert not database.exists()
    run('init', 'Empty', structured=True)
    assert run('status', '--no-simulation', structured=True)['total_work'] == 0
    assert run('next', structured=True)['candidates'] == []
    return {'passed': len(checks), 'operations': count, 'final_status': final, 'checks': checks}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/dpm')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='dpm-smoke-') as temporary:
        report = smoke(args.binary.resolve(), Path(temporary))
    if args.report:
        args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"PASS: {report['passed']} CLI checks; {report['operations']} persisted operations")


if __name__ == '__main__':
    main()
