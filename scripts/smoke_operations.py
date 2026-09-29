#!/usr/bin/env python3
"""Client operation identities, lineage preconditions and busy stores behave alike through CLI and MCP."""
import os
import secrets
import sqlite3
import tempfile
import time
import uuid
from pathlib import Path

import json
import subprocess

from smoke_agent import CLI, ROOT, Agent, run_cli, run_cli_envelope

FIXTURE = ROOT / 'tests/support/execution-plan.json'
ACTOR = 'agent:ops'


def uuid7():
    """A version 7 UUID: a millisecond timestamp, then random bits, with version and variant set."""
    value = (int(time.time() * 1000) << 80) | secrets.randbits(80)
    value = (value & ~(0xF << 76)) | (0x7 << 76)
    value = (value & ~(0x3 << 62)) | (0x2 << 62)
    return str(uuid.UUID(int=value))


def same_refusal(agent, tool, arguments, database, command, code):
    """Both adapters refuse alike, with the same error object, and nothing changes."""
    before = run_cli(database, 'export')
    remote = agent.call(tool, arguments, error=code)
    local = run_cli(database, *command, error=code)['error']
    assert remote == local, (remote, local)
    assert run_cli(database, 'export') == before
    return local


def resend(database, agent):
    """A resent identity answers the recorded operation through either adapter; other content is refused."""
    status = run_cli_envelope(database, 'status', '--no-simulation')
    lineage = status['lineage_id']
    assert lineage and status['data']['lineage_id'] == lineage, status
    assert agent.call('project_status', {'probabilistic': False})['lineage_id'] == lineage
    identity = uuid7()
    first = run_cli_envelope(database, '--operation-id', identity, 'claim', 'TEST-A', '--actor', ACTOR)
    recorded = first['data']
    assert recorded['id'] == identity and first['lineage_id'] == recorded['lineage_id'] == lineage, first
    assert recorded['workspace_id'] == run_cli(database, 'export')['workspace']['id'], recorded
    # A retry carries the revision it first observed; the identity is answered before the revision.
    again = run_cli_envelope(database, '--base-revision', '0', '--operation-id', identity, 'claim', 'TEST-A',
                             '--actor', ACTOR)
    assert again == first, (again, first)
    remote = agent.call('claim_work', {'key': 'TEST-A', 'base_revision': 0, 'operation_id': identity})
    assert remote == first, (remote, first)
    duplicate = same_refusal(agent, 'start_work', {'key': 'TEST-A', 'base_revision': 1, 'operation_id': identity},
                             database, ('--operation-id', identity, 'start', 'TEST-A', '--actor', ACTOR),
                             'duplicate_operation')
    assert duplicate['details']['recorded'] == recorded, duplicate
    for invalid in (str(uuid.uuid4()), 'not-a-uuid'):
        agent.call('start_work', {'key': 'TEST-A', 'base_revision': 1, 'operation_id': invalid},
                   error='invalid_request')
        run_cli(database, '--operation-id', invalid, 'start', 'TEST-A', '--actor', ACTOR, error='invalid_request')
    page = run_cli_envelope(database, 'history')
    assert page['lineage_id'] == page['data']['lineage_id'] == lineage, page
    assert page['data']['entries'][-1]['operation'] == recorded, page
    assert agent.call('history', {})['data'] == page['data']
    return lineage


def lineage_precondition(database, agent, lineage):
    """A write names the lineage its revision was observed in; another lineage is refused alike."""
    other = uuid7()
    refused = same_refusal(agent, 'start_work', {'key': 'TEST-A', 'base_revision': 1, 'base_lineage': other},
                           database, ('--base-revision', '1', '--base-lineage', other, 'start', 'TEST-A',
                                      '--actor', ACTOR), 'lineage_mismatch')
    assert refused['details'] == {'expected': other, 'actual': lineage}, refused
    started = agent.call('start_work', {'key': 'TEST-A', 'base_revision': 1, 'base_lineage': lineage})
    assert started['lineage_id'] == lineage and started['revision'] == 2, started


def restored_copy(directory, database, identity_of_claim):
    """A restored copy continues under a new lineage and still answers identities it copied."""
    backup, restored = directory / 'ops-backup.sqlite', directory / 'ops-restored.sqlite'
    source = run_cli(database, 'backup', '--to', str(backup))
    restore = run_cli_envelope(database, 'restore', '--from', str(backup), '--to', str(restored))
    report = restore['data']
    assert restore['lineage_id'] == report['lineage_id'], restore
    assert source['archived'] and not report['archived'] and report['lineage_id'] != source['lineage_id']
    agent = Agent(restored, ACTOR)
    try:
        again = agent.call('claim_work', {'key': 'TEST-A', 'base_revision': 0, 'operation_id': identity_of_claim})
        assert again['data']['lineage_id'] == source['lineage_id'], again
        assert again['lineage_id'] == source['lineage_id'], 'the envelope names the recorded lineage'
        same_refusal(agent, 'report_progress',
                     {'key': 'TEST-A', 'percent': 10, 'base_revision': 3, 'base_lineage': source['lineage_id']},
                     restored, ('--base-revision', '3', '--base-lineage', source['lineage_id'], 'progress',
                                'TEST-A', '10', '--actor', ACTOR), 'lineage_mismatch')
        progressed = agent.call('report_progress', {'key': 'TEST-A', 'percent': 10, 'base_revision': 3,
                                                    'base_lineage': report['lineage_id']})
        assert progressed['lineage_id'] == report['lineage_id'], progressed
    finally:
        agent.close()
    same_refusal_archive(backup)


def same_refusal_archive(backup):
    before = backup.read_bytes()
    agent = Agent(backup, ACTOR)
    try:
        agent.call('report_progress', {'key': 'TEST-A', 'percent': 10, 'base_revision': 2}, error='archived_store')
    finally:
        agent.close()
    run_cli(backup, 'progress', 'TEST-A', '10', '--actor', ACTOR, error='archived_store')
    assert backup.read_bytes() == before


def busy(database, agent):
    """A write lock held past the store's timeout is the retryable store_busy through both adapters."""
    holder = sqlite3.connect(database, isolation_level=None)
    try:
        holder.execute('BEGIN IMMEDIATE')
        identity = uuid7()
        run_cli(database, '--operation-id', identity, 'progress', 'TEST-A', '20', '--actor', ACTOR,
                error='store_busy')
        agent.call('report_progress', {'key': 'TEST-A', 'percent': 20, 'base_revision': 2,
                                       'operation_id': identity}, error='store_busy')
    finally:
        holder.execute('ROLLBACK')
        holder.close()
    retried = run_cli(database, '--operation-id', identity, 'progress', 'TEST-A', '20', '--actor', ACTOR)
    assert retried['id'] == identity and retried['resulting_revision'] == 3, retried


def rebuilt_and_raced(directory):
    """A committed identity is never reported as failed: not when its request can no longer be
    rebuilt from the live state, and not when parallel attempts race with it."""
    database = directory / 'rebuilt.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    agent = Agent(database, ACTOR)
    try:
        run_cli(database, 'claim', 'TEST-A', '--actor', ACTOR)
        flags = ['--provider', 'Forgejo', '--instance', 'git.alpha.example', '--namespace', 'ops/dpm', '--id', '42']
        identity = {'provider': 'Forgejo', 'instance': 'git.alpha.example', 'namespace': 'ops/dpm', 'kind': 'Issue',
                    'external_id': '42'}
        run_cli(database, 'link-external', 'TEST-A', *flags, '--actor', ACTOR)
        operation = uuid7()
        unlinked = run_cli(database, '--operation-id', operation, 'unlink-external', 'TEST-A', *flags, '--actor', ACTOR)
        # The reference is gone, so the unlink cannot be rebuilt; the resend reports the record.
        revision = unlinked['resulting_revision']
        duplicate = same_refusal(agent, 'unlink_external', {'key': 'TEST-A', 'identity': identity,
                                                            'base_revision': revision, 'operation_id': operation},
                                 database, ('--operation-id', operation, 'unlink-external', 'TEST-A', *flags,
                                            '--actor', ACTOR), 'duplicate_operation')
        assert duplicate['details']['recorded'] == unlinked, duplicate
    finally:
        agent.close()
    run_cli(database, 'start', 'TEST-A', '--actor', ACTOR)
    for percent in range(10, 90, 10):
        identity = uuid7()
        command = [str(CLI), '--database', str(database), '--json', '--operation-id', identity,
                   'progress', 'TEST-A', str(percent), '--actor', ACTOR]
        racers = [subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                  for _ in range(6)]
        answers = [json.loads(racer.communicate(timeout=30)[0]) for racer in racers]
        codes = {answer.get('error', {}).get('code', 'ok') for answer in answers}
        assert codes == {'ok'}, (percent, codes)
        assert len({json.dumps(answer['data'], sort_keys=True) for answer in answers}) == 1, answers


def operations_smoke(directory):
    database = directory / 'operations.sqlite'
    imported = run_cli_envelope(database, 'import', str(FIXTURE))
    agent = Agent(database, ACTOR)
    try:
        lineage = resend(database, agent)
        # Import reports the lineage it started, for the client's first write at revision 0.
        assert imported['lineage_id'] == imported['data']['lineage_id'] == lineage, imported
        claim = run_cli(database, 'history')['entries'][-1]['operation']['id']
        lineage_precondition(database, agent, lineage)
        busy(database, agent)
    finally:
        agent.close()
    restored_copy(directory, database, claim)
    rebuilt_and_raced(directory)
    print('PASS: CLI/MCP operation ids: recorded resends, duplicate and invalid ids, lineage preconditions, '
          'archives, restored lineages, retryable busy stores, unrebuildable resends and same-identity races')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-operations-') as temporary:
        os.environ['DPM_CONFIG_DIR'] = str(Path(temporary) / 'config')
        operations_smoke(Path(temporary))
