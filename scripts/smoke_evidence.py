#!/usr/bin/env python3
"""Evidence is a reference: both adapters attach a local file by location, digest and size without
copying it, and refuse evidence that carries content or cannot be checked later, alike."""
import json
import re
import uuid
from datetime import datetime, timezone

from smoke_agent import ROOT, Agent, run_cli
from smoke_ownership import same_refusal

DIGEST = '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08'


def refused_file(agent, database, path, kind, extra, flags, code='invalid_command'):
    """Both adapters refuse the same file evidence alike and nothing changes; each mints its own
    artifact identity, which is the only difference the messages may show."""
    before = run_cli(database, 'export')
    remote = agent.call('attach_file', {'key': 'TEST-A', 'path': str(path), 'kind': kind, 'label': 'Run', **extra,
                                        'base_revision': before['revision']}, error=code)['message']
    local = run_cli(database, 'attach-file', 'TEST-A', str(path), '--kind', kind, '--label', 'Run', *flags,
                    '--actor', 'agent:parity', error=code)['error']['message']
    minted = re.compile(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}')
    assert minted.sub('ID', remote) == minted.sub('ID', local), (remote, local)
    assert run_cli(database, 'export') == before
    return local


def evidence_smoke(directory):
    database = directory / 'evidence-files.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    try:
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})
        report = directory / 'acceptance report.txt'
        report.write_text('test')
        remote = worker.call('attach_file', {'key': 'TEST-A', 'path': str(report), 'kind': 'TestResult', 'label': 'Acceptance',
                                             'outcome': 'passed', 'metadata': {'command': 'make check'}, 'base_revision': 1})['data']
        local = run_cli(database, 'attach-file', 'TEST-A', str(report), '--kind', 'TestResult', '--label', 'Acceptance',
                        '--outcome', 'passed', '--meta', 'command=make check', '--actor', 'agent:parity')
        attached = [operation['command']['AttachArtifact']['artifact'] for operation in (remote, local)]
        expected = {'bytes': '4', 'command': 'make check', 'outcome': 'passed', 'sha256': DIGEST}
        assert all(a['uri'] == report.resolve().as_uri() and a['metadata'] == expected for a in attached), attached
        assert report.read_text() == 'test', 'the file is read, never moved or rewritten'

        unstated = refused_file(worker, database, report, 'Build', {}, [])
        assert 'outcome' in unstated, unstated
        measured = refused_file(worker, database, report, 'File', {'metadata': {'sha256': DIGEST}}, ['--meta', f'sha256={DIGEST}'],
                                code='invalid_request')
        assert 'measured' in measured, measured
        stored = directory / 'project/.dpm/artifacts/run.log'
        stored.parent.mkdir(parents=True)
        stored.write_text('log')
        inside = refused_file(worker, database, stored, 'File', {}, [])
        assert 'store directory' in inside, inside
        for uri, metadata, why in [('data:text/plain;base64,dGVzdA==', {}, 'carries content'),
                                   ('file:///Volumes/QA/inspection.jpg', {}, 'sha256')]:
            artifact = {'id': str(uuid.uuid4()), 'kind': 'Photo', 'uri': uri, 'label': 'Inspection', 'metadata': metadata,
                        'created_by': {'kind': 'Agent', 'name': 'parity'}, 'created_at': datetime.now(timezone.utc).isoformat()}
            path = directory / f"{artifact['id']}.json"
            path.write_text(json.dumps(artifact))
            message = same_refusal(worker, 'add_artifact', {'key': 'TEST-A', 'artifact': artifact}, database,
                                   ['artifact', 'TEST-A', str(path), '--actor', 'agent:parity'])
            assert why in message, message
    finally:
        worker.close()
    print('PASS: CLI/MCP evidence files by location, SHA-256 and size; content, unstated outcomes and store copies refused alike')
