#!/usr/bin/env python3
"""Check Microsoft Project XML import/export through the real CLI and agent transport."""
import json
import re
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


def without_guids(xml):
    """The document as a GUID-dropping tool (OmniPlan) writes it back."""
    return re.sub(r'<GUID>[^<]*</GUID>', '', xml)


def check_imports(database, worker):
    """Both adapters return the same candidate and report, and importing changes nothing."""
    before, count = run_cli(database, 'export'), operations(database)
    for fixture in ['mpxj-release-plan.xml', 'mpxj-unsupported-features.xml', 'omniplan-native.xml', 'omniplan-metadata-roundtrip.xml', 'mpxj-metadata-roundtrip.xml']:
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
             (RELEASE.read_text(), 'NOPE', 'not_found'),
             (without_guids(RELEASE.read_text()), 'TEST', 'invalid_request')]
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
    check_guidless_round_trip(database, worker, directory, local['xml'])
    metadata_xml = without_guids(local['xml']).replace('Write specification', 'Renamed in external scheduler')
    written.write_text(metadata_xml)
    args = {'xml': metadata_xml, 'project_key': 'TEST'}
    imported_metadata = run_cli(database, 'plan', 'import-mspdi', str(written), '--project-key', 'TEST')
    assert imported_metadata == worker.call('import_mspdi', args)['data']
    assert all(i['outcome'] != 'Created' for i in imported_metadata['report']['items'])
    assert any(i['name'] == 'Renamed in external scheduler' for i in imported_metadata['report']['items'])
    imported = {w['key'] for w in run_cli(database, 'export')['work_items'].values() if w['key'].startswith('MSP-')}
    assert imported and not imported & {c['work']['key'] for c in worker.call('next_work', {'limit': 100})['data']['candidates']}


def check_guidless_round_trip(database, worker, directory, exported):
    """A GUID-less copy of DPM's export maps back onto the same work only when asked, identically in both adapters."""
    returned = directory / 'returned.xml'
    returned.write_text(re.sub(r'<ExtendedAttributes>.*?</ExtendedAttributes>|<ExtendedAttribute>.*?</ExtendedAttribute>', '', without_guids(exported), flags=re.S))
    arguments = {'xml': returned.read_text(), 'project_key': 'TEST', 'key_prefix': 'OPR'}
    remote = worker.call('import_mspdi', {**arguments, 'match_existing_by': 'title-path'})['data']
    local = run_cli(database, 'plan', 'import-mspdi', str(returned), '--project-key', 'TEST', '--key-prefix', 'OPR',
                    '--match-existing-by', 'title-path')
    assert remote == local and local['preview']['changes'] == [], local['preview']
    assert all(i['approximated'][0]['detail'].startswith('no task GUID; matched existing work ') for i in local['report']['items'])
    separate = run_cli(database, 'plan', 'import-mspdi', str(returned), '--project-key', 'TEST', '--key-prefix', 'OPR')
    assert separate == worker.call('import_mspdi', arguments)['data']
    assert {i['outcome'] for i in separate['report']['items']} == {'Created'}
    bad = worker.call('import_mspdi', {**arguments, 'match_existing_by': 'fuzzy'}, error='invalid_request')
    assert 'fuzzy' in bad['message'] or 'title-path' in bad['message'], bad


def check_rescaled_priorities(directory):
    """OmniPlan rescales priorities by the highest one; keeping local priority is explicit and identical in both adapters."""
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    for item, band in zip(sorted(plan['work_items'].values(), key=lambda w: w['key']), ['P1', 'P2', 'P3'] * 10):
        item['schedule']['priority'] = band
    source, database = directory / 'no-p0.json', directory / 'no-p0.sqlite'
    source.write_text(json.dumps(plan))
    run_cli(database, 'import', str(source))
    fixture = ROOT / 'crates/dpm-interchange/tests/mspdi/omniplan-export-no-p0.xml'
    arguments = {'xml': fixture.read_text(), 'project_key': 'TEST', 'key_prefix': 'OPR', 'match_existing_by': 'title-path'}
    flags = ['plan', 'import-mspdi', str(fixture), '--project-key', 'TEST', '--key-prefix', 'OPR', '--match-existing-by', 'title-path']
    worker = Agent(database, 'agent:importer')
    try:
        raised = run_cli(database, *flags)
        assert raised == worker.call('import_mspdi', arguments)['data']
        assert {(c['before'], c['after']) for i in raised['report']['items'] for c in i['changes']} == {('P1', 'P0'), ('P2', 'P1'), ('P3', 'P2')}
        kept = run_cli(database, *flags, '--keep-existing-priority')
        assert kept == worker.call('import_mspdi', {**arguments, 'keep_existing_priority': True})['data']
        assert kept['preview']['changes'] == [], kept['preview']
        assert all('priority' in i['kept'] for i in kept['report']['items'][1:])
    finally:
        worker.close()


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
    check_rescaled_priorities(directory)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory() as directory:
        smoke(Path(directory))
    print('PASS: MSPDI import/export CLI/MCP parity, reports keep imported tasks Proposed, failed imports and refused applies change nothing, human apply, export re-imports without changes, carried metadata preserves renamed GUID-less work and O/M/P; files without metadata support explicit title-path matching, rescaled OmniPlan priorities stay local only with --keep-existing-priority')
