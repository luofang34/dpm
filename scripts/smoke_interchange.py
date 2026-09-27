#!/usr/bin/env python3
"""Check Microsoft Project XML import/export through the real CLI and agent transport."""
import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from smoke_agent import Agent, CLI, ROOT, run_cli

FIXTURES = ROOT / 'crates/dpm-interchange/tests/mspdi'
RELEASE = FIXTURES / 'mpxj-release-plan.xml'


def operations(database):
    with sqlite3.connect(database) as connection:
        return connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0]


def import_args(path, *extra):
    return ('plan', 'import-mspdi', str(path), '--project-key', 'TEST', '--key-prefix', 'MSP', *extra)


def check_imports(database, worker):
    """Both adapters return the same candidate and report, and importing changes nothing."""
    before, count = run_cli(database, 'export'), operations(database)
    for fixture in ['mpxj-release-plan.xml', 'mpxj-unsupported-features.xml']:
        remote = worker.call('import_mspdi', {'xml': (FIXTURES / fixture).read_text(), 'project_key': 'TEST', 'key_prefix': 'MSP'})
        local = run_cli(database, *import_args(FIXTURES / fixture))
        assert remote['data'] == local and remote['revision'] == before['revision'], fixture
        assert local['preview']['base_revision'] == before['revision']
        assert 'not calendar dates' in local['report']['source']['scope'], fixture
        for item in local['report']['items']:
            work = item['work']
            assert work is None or work['status'] in ('Proposed', 'Planned'), item
    unsupported = run_cli(database, *import_args(FIXTURES / 'mpxj-unsupported-features.xml'))['report']
    finished = next(i for i in unsupported['items'] if i['name'] == 'Finished task')
    assert finished['work']['status'] == 'Proposed'
    assert {'PercentComplete', 'actuals'} <= {f['field'] for f in finished['rejected']}
    assert run_cli(database, 'export') == before and operations(database) == count


def check_failures(database, worker, directory):
    """Failed imports return the same code and message from both adapters and change nothing."""
    before, count = run_cli(database, 'export'), operations(database)
    cases = [('<Project/>', 'TEST', 'invalid_request'), ('<unterminated', 'TEST', 'invalid_request'),
             (RELEASE.read_text(), 'NOPE', 'not_found')]
    for index, (xml, project, code) in enumerate(cases):
        path = directory / f'bad-{index}.xml'
        path.write_text(xml)
        remote = worker.call('import_mspdi', {'xml': xml, 'project_key': project}, error=code)
        local = run_cli(database, 'plan', 'import-mspdi', str(path), '--project-key', project, error=code)
        assert remote['message'] == local['error']['message'], (remote, local)
    remote = worker.call('export_mspdi', {'project_key': 'NOPE'}, error='not_found')
    local = run_cli(database, 'plan', 'export-mspdi', '--project-key', 'NOPE', error='not_found')
    assert remote['message'] == local['error']['message']
    assert run_cli(database, 'export') == before and operations(database) == count


def check_apply_and_round_trip(database, worker, directory):
    """Only a human/service applies the candidate; the export re-imports without changes."""
    before, count = run_cli(database, 'export'), operations(database)
    candidate = directory / 'candidate.json'
    run_cli(database, *import_args(RELEASE, '--candidate', str(candidate)))
    plan = json.loads(candidate.read_text())
    worker.call('apply_change', {'plan': plan, 'reason': 'self-approve', 'base_revision': before['revision']}, error='invalid_command')
    assert operations(database) == count
    applied = run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Import the reviewed release schedule', '--actor', 'human:reviewer')
    assert applied['resulting_revision'] == before['revision'] + 1 and operations(database) == count + 1
    again = worker.call('import_mspdi', {'xml': RELEASE.read_text(), 'project_key': 'TEST', 'key_prefix': 'MSP'})['data']
    assert again['preview']['changes'] == []
    remote = worker.call('export_mspdi', {'project_key': 'TEST'})
    local = run_cli(database, 'plan', 'export-mspdi', '--project-key', 'TEST')
    assert remote['data'] == local and remote['revision'] == applied['resulting_revision']
    raw = subprocess.run([str(CLI), '--database', str(database), 'plan', 'export-mspdi', '--project-key', 'TEST'],
                         cwd=ROOT, capture_output=True, text=True, timeout=15, check=True).stdout
    assert raw == local['xml']
    written = directory / 'export.xml'
    run_cli(database, 'plan', 'export-mspdi', '--project-key', 'TEST', '--output', str(written))
    assert written.read_text() == local['xml']
    assert run_cli(database, 'plan', 'import-mspdi', str(written), '--project-key', 'TEST')['preview']['changes'] == []
    imported = {w['key'] for w in run_cli(database, 'export')['work_items'].values() if w['key'].startswith('MSP-')}
    assert imported and not imported & {c['work']['key'] for c in worker.call('next_work', {'limit': 100})['data']['candidates']}


def smoke(directory):
    database = directory / 'interchange.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:importer')
    try:
        tools = {tool['name']: tool for tool in worker.request('tools/list', {})['tools']}
        for name in ('import_mspdi', 'export_mspdi'):
            assert tools[name]['annotations']['readOnlyHint'] is True
        check_imports(database, worker)
        check_failures(database, worker, directory)
        check_apply_and_round_trip(database, worker, directory)
    finally:
        worker.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory() as directory:
        smoke(Path(directory))
    print('PASS: MSPDI import/export CLI/MCP parity, reports keep imported tasks Proposed, failed imports and refused applies change nothing, human apply, export re-imports without changes')
