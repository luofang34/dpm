#!/usr/bin/env python3
"""Agent runs through the real CLI and agent-tool processes: one contract, no project side effects."""
import json
import re
import subprocess
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from smoke_agent import CLI, ROOT, Agent, run_cli, run_cli_envelope

FIXTURE = ROOT / 'tests/support/execution-plan.json'
TIME = re.compile(r'^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}')
RUN = '0192f000-0000-7000-8000-0000000000a1'
WORKER = 'agent:worker'


def normalize(value, aliases):
    """The same structure with receipt times and per-store identities replaced, so two stores that
    were driven identically compare equal."""
    if isinstance(value, dict):
        return {key: normalize(item, aliases) for key, item in value.items()}
    if isinstance(value, list):
        return [normalize(item, aliases) for item in value]
    if isinstance(value, str):
        return aliases.get(value) or ('<time>' if TIME.match(value) else value)
    return value


class Twin:
    """Two stores imported from one plan: the CLI drives one and the agent tools the other."""

    def __init__(self, directory):
        self.cli_db, self.mcp_db = directory / 'runs-cli.sqlite', directory / 'runs-mcp.sqlite'
        for database in (self.cli_db, self.mcp_db):
            run_cli(database, 'import', str(FIXTURE))
        self.agent = Agent(self.mcp_db, WORKER)
        self.aliases = {}
        for tool, command in [('claim_work', 'claim'), ('start_work', 'start')]:
            revision = run_cli(self.cli_db, 'revision')['revision']
            run_cli(self.cli_db, command, 'TEST-A', '--actor', WORKER)
            self.agent.call(tool, {'key': 'TEST-A', 'base_revision': revision})
        for name, database in (('<lineage>', self.cli_db), ('<lineage>', self.mcp_db)):
            self.aliases[run_cli(database, 'revision')['lineage_id']] = name

    def both(self, tool, arguments, *command, error=None):
        """One run command through each adapter; the results must be the same, and are returned."""
        local = run_cli_envelope(self.cli_db, *command, error=error)
        remote = self.agent.call(tool, arguments, error=error)
        if error:
            assert normalize(local['error'], self.aliases) == normalize(remote, self.aliases), (tool, local, remote)
            return remote
        assert normalize(local, self.aliases) == normalize(remote, self.aliases), (tool, local, remote)
        return local

    def close(self):
        self.agent.close()


def project_unchanged(twin, revision, operations):
    """Run commands never move the project: revision, history and ownership stay where they were."""
    for database in (twin.cli_db, twin.mcp_db):
        assert run_cli(database, 'revision')['revision'] == revision
        assert len(run_cli(database, 'history')['entries']) == operations
        work = run_cli(database, 'show', 'TEST-A')
        assert work['execution']['owner'] == {'kind': 'Agent', 'name': 'worker'}, work['execution']


def lifecycle_parity(twin):
    session = {'provider': 'codex', 'session': 'thread-1'}
    started = twin.both('start_run', {'key': 'TEST-A', 'run_id': RUN, 'session': session},
                        'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', RUN,
                        '--provider', 'codex', '--session', 'thread-1')
    data = started['data']
    assert started['revision'] == 3 and data['replayed'] is False, started
    assert data['run']['status'] == 'working' and data['run']['run']['observation'] == 'reported_only', data
    assert data['run']['run']['contract']['revision'] == 3 and data['run']['run']['contract']['work_key'] == 'TEST-A'
    again = twin.both('start_run', {'key': 'TEST-A', 'run_id': RUN, 'session': session},
                      'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', RUN,
                      '--provider', 'codex', '--session', 'thread-1')
    assert again['data']['replayed'] is True and again['data']['run']['run'] == data['run']['run']
    conflict = twin.both('start_run', {'key': 'TEST-A', 'run_id': RUN, 'session': {**session, 'turn': 'turn-2'}},
                         'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', RUN,
                         '--provider', 'codex', '--session', 'thread-1', '--turn', 'turn-2',
                         error='duplicate_run_record')
    assert conflict['details']['recorded']['id'] == RUN
    # Only a service may label a run managed: nothing else can observe the executor.
    managed = '0192f000-0000-7000-8000-0000000000a9'
    twin.both('start_run', {'key': 'TEST-A', 'run_id': managed, 'observation': 'managed'},
              'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', managed, '--observation', 'managed',
              error='invalid_command')
    # A run identity must be a version 7 UUID, whichever adapter carries it.
    version4 = '0192f000-0000-4000-8000-0000000000a1'
    twin.both('start_run', {'key': 'TEST-A', 'run_id': version4},
              'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', version4, error='invalid_request')


def source_parity(twin, directory):
    """Only exact commits are sources: a branch-named artifact is refused by both adapters."""
    branch = {'id': '0192f000-0000-4000-8000-0000000000b1', 'kind': 'GitCommit', 'uri': 'git:local@main',
              'label': 'mutable ref', 'metadata': {}, 'created_by': {'kind': 'Agent', 'name': 'worker'},
              'created_at': '2026-09-01T00:00:00Z'}
    path = directory / 'branch-artifact.json'
    path.write_text(json.dumps(branch))
    revision = run_cli(twin.cli_db, 'revision')['revision']
    run_cli(twin.cli_db, 'artifact', 'TEST-A', str(path), '--actor', WORKER)
    twin.agent.call('add_artifact', {'key': 'TEST-A', 'artifact': branch, 'base_revision': revision})
    local = run_cli(twin.cli_db, 'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id',
                    '0192f000-0000-7000-8000-0000000000b2', '--source-artifact', branch['id'], error='invalid_command')
    remote = twin.agent.call('start_run', {'key': 'TEST-A', 'run_id': '0192f000-0000-7000-8000-0000000000b2',
                                           'sources': [{'kind': 'artifact', 'artifact': branch['id']}]},
                             error='invalid_command')
    assert normalize(local['error'], twin.aliases) == normalize(remote, twin.aliases)
    assert 'not a full commit' in local['error']['message'] or 'commit' in local['error']['message'], local


def activity_parity(twin):
    heartbeat = lambda: twin.both('record_run_activity', {'run': RUN, 'source_sequence': 1, 'kind': 'heartbeat'},
                                  'run', 'record', RUN, 'heartbeat', '--sequence', '1', '--actor', WORKER)
    first = heartbeat()
    assert first['data']['entries'][0]['duplicate'] is False and first['data']['entries'][0]['sequence'] == 1
    assert heartbeat()['data']['entries'][0]['duplicate'] is True, 'duplicate delivery is harmless'
    twin.both('record_run_activity', {'run': RUN, 'source_sequence': 1, 'kind': 'progress', 'text': 'changed'},
              'run', 'record', RUN, 'progress', '--sequence', '1', '--text', 'changed', '--actor', WORKER,
              error='duplicate_run_record')
    long = 'x' * 5000
    stored = twin.both('record_run_activity', {'run': RUN, 'source_sequence': 2, 'kind': 'tool_result', 'text': long},
                       'run', 'record', RUN, 'tool_result', '--sequence', '2', '--text', long, '--actor', WORKER)
    entry = stored['data']['entries'][0]
    assert entry['truncated'] is True and len(entry['text']) == 4096 and len(entry['text_digest']) == 64, entry
    # The whole text is part of the identity: a difference past the kept 4096 bytes is a conflict.
    changed = long[:-1] + 'y'
    twin.both('record_run_activity', {'run': RUN, 'source_sequence': 2, 'kind': 'tool_result', 'text': changed},
              'run', 'record', RUN, 'tool_result', '--sequence', '2', '--text', changed, '--actor', WORKER,
              error='duplicate_run_record')
    twin.both('record_run_activity', {'run': RUN, 'source_sequence': 5, 'kind': 'heartbeat'},
              'run', 'record', RUN, 'heartbeat', '--sequence', '5', '--actor', WORKER)
    # A sequence at or below the high-water mark that is not held cannot be told from a retry that
    # retention removed, so it is refused as expired and records nothing.
    expired = twin.both('record_run_activity', {'run': RUN, 'source_sequence': 3, 'kind': 'heartbeat'},
                        'run', 'record', RUN, 'heartbeat', '--sequence', '3', '--actor', WORKER,
                        error='activity_expired')
    assert expired['details']['high_water'] == 5 and expired['details']['source_sequence'] == 3
    twin.both('record_run_activity', {'run': RUN, 'source_sequence': 0, 'kind': 'heartbeat'},
              'run', 'record', RUN, 'heartbeat', '--sequence', '0', '--actor', WORKER, error='invalid_request')
    twin.both('record_run_activity', {'run': '0192f000-0000-7000-8000-0000000000ee', 'source_sequence': 1, 'kind': 'heartbeat'},
              'run', 'record', '0192f000-0000-7000-8000-0000000000ee', 'heartbeat', '--sequence', '1',
              '--actor', WORKER, error='not_found')


def freshness_parity(twin):
    """Silence is stale on the pinned query clock, never finished; both adapters agree exactly."""
    later = (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat()
    local = run_cli_envelope(twin.cli_db, '--clock', later, 'run', 'show', RUN)['data']
    agent = Agent(twin.mcp_db, WORKER, clock=later)
    try:
        remote = agent.call('get_run', {'run': RUN})['data']
    finally:
        agent.close()
    assert local['status'] == remote['status'] == 'stale', (local['status'], remote['status'])
    assert local['state'] == remote['state'] == 'working' and 'stale_at' not in local
    assert normalize(local, twin.aliases) == normalize(remote, twin.aliases)
    now = run_cli(twin.cli_db, 'run', 'show', RUN)
    assert now['status'] == 'working' and now['stale_at'], now
    run_cli(twin.cli_db, '--clock', later, 'run', 'list', error=None)
    refused = run_cli(twin.cli_db, '--clock', later, 'run', 'record', RUN, 'heartbeat', '--sequence', '50',
                      '--actor', WORKER, error='invalid_request')
    assert 'queries' in refused['error']['message']


def link_and_finish_parity(twin):
    revision = run_cli(twin.cli_db, 'revision')['revision']
    submitted = run_cli(twin.cli_db, 'submit', 'TEST-A', '--actor', WORKER)
    remote = twin.agent.call('submit_work', {'key': 'TEST-A', 'base_revision': revision})['data']
    twin.aliases[submitted['id']] = twin.aliases[remote['id']] = '<operation>'
    local_link = run_cli_envelope(twin.cli_db, 'run', 'link', RUN, submitted['id'], '--actor', WORKER)
    remote_link = twin.agent.call('link_run_operation', {'run': RUN, 'operation': remote['id']})
    assert normalize(local_link, twin.aliases) == normalize(remote_link, twin.aliases)
    assert local_link['data']['link']['operation'] == submitted['id'] and local_link['data']['replayed'] is False
    assert run_cli(twin.cli_db, 'run', 'link', RUN, submitted['id'], '--actor', WORKER)['replayed'] is True
    run_cli(twin.cli_db, 'run', 'link', RUN, '0192f000-0000-7000-8000-0000000000ff', '--actor', WORKER, error='not_found')
    # The independent review is not the executor's operation and cannot be attributed to the run.
    revision = run_cli(twin.cli_db, 'revision')['revision']
    verified = run_cli(twin.cli_db, 'verify', 'TEST-A', '--actor', 'human:reviewer')
    reviewer = Agent(twin.mcp_db, 'human:reviewer')
    try:
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': revision})
    finally:
        reviewer.close()
    refused = run_cli(twin.cli_db, 'run', 'link', RUN, verified['id'], '--actor', WORKER, error='run_link_refused')
    assert 'executor' in refused['error']['details']['reason']
    assert run_cli(twin.cli_db, 'show', 'TEST-A')['execution']['status'] == 'Verified'
    return revision + 1


def report_parity(twin):
    waiting = '0192f000-0000-7000-8000-0000000000b1'
    done = '0192f000-0000-7000-8000-0000000000b2'
    twin.both('report_run', {'run': RUN, 'state': 'waiting', 'event_id': waiting, 'detail': 'needs input'},
              'run', 'report', RUN, 'waiting', '--event-id', waiting, '--detail', 'needs input', '--actor', WORKER)
    replay = twin.both('report_run', {'run': RUN, 'state': 'waiting', 'event_id': waiting, 'detail': 'needs input'},
                       'run', 'report', RUN, 'waiting', '--event-id', waiting, '--detail', 'needs input', '--actor', WORKER)
    assert replay['data']['replayed'] is True
    twin.both('report_run', {'run': RUN, 'state': 'failed', 'event_id': waiting},
              'run', 'report', RUN, 'failed', '--event-id', waiting, '--actor', WORKER, error='duplicate_run_record')
    twin.both('report_run', {'run': RUN, 'state': 'waiting'},
              'run', 'report', RUN, 'waiting', '--actor', WORKER, error='invalid_run_transition')
    twin.both('report_run', {'run': RUN, 'state': 'completed', 'event_id': done},
              'run', 'report', RUN, 'completed', '--event-id', done, '--actor', WORKER)
    twin.both('report_run', {'run': RUN, 'state': 'working'},
              'run', 'report', RUN, 'working', '--actor', WORKER, error='invalid_run_transition')


def read_parity(twin):
    for tool, arguments, command in [
        ('get_run', {'run': RUN}, ('run', 'show', RUN)),
        ('list_runs', {'key': 'TEST-A'}, ('run', 'list', '--key', 'TEST-A')),
        ('list_runs', {}, ('run', 'list')),
        ('run_lifecycle', {}, ('run', 'lifecycle')),
        ('run_lifecycle', {'after_sequence': 1, 'limit': 1, 'run': RUN}, ('run', 'lifecycle', '--after-sequence', '1', '--limit', '1', '--run', RUN)),
        ('run_activity', {}, ('run', 'activity')),
        ('run_activity', {'after_sequence': 1, 'limit': 5, 'run': RUN}, ('run', 'activity', '--after-sequence', '1', '--limit', '5', '--run', RUN)),
    ]:
        local = run_cli_envelope(twin.cli_db, *command)
        remote = twin.agent.call(tool, arguments)
        # Reads of one run at two clocks differ only in the evaluation instant, which is normalized.
        assert normalize(local, twin.aliases) == normalize(remote, twin.aliases), (tool, local, remote)
    shown = run_cli(twin.cli_db, 'run', 'show', RUN)
    assert shown['status'] == 'completed' and shown['operations'][0]['work'] == shown['run']['work']
    assert shown['activity'] == {'recorded': 3, 'retained': 3, 'source_high_water': 5, 'latest': shown['activity']['latest']}
    lifecycle = run_cli(twin.cli_db, 'run', 'lifecycle')
    assert [e['state'] for e in lifecycle['entries']] == ['working', 'waiting', 'completed'] and lifecycle['head_sequence'] == 3
    tail = run_cli(twin.cli_db, 'run', 'lifecycle', '--after-sequence', '3')
    assert tail['entries'] == [] and tail['next_after_sequence'] == 3, 'an empty page keeps the cursor'
    big = next(e for e in run_cli(twin.cli_db, 'run', 'activity')['entries'] if e['source_sequence'] == 2)
    assert big['truncated'] is True and len(big['text']) == 4096 and len(big['text_digest']) == 64, big


def preconditions(twin):
    """Project preconditions do not apply to runs, and a wrong lineage is refused with nothing written."""
    refused = run_cli(twin.cli_db, '--base-revision', '3', 'run', 'record', RUN, 'heartbeat', '--sequence', '9',
                      '--actor', WORKER, error='invalid_request')
    assert 'base-revision' in refused['error']['message']
    run_cli(twin.cli_db, '--operation-id', '0192f000-0000-7000-8000-0000000000c1', 'run', 'record', RUN,
            'heartbeat', '--sequence', '9', '--actor', WORKER, error='invalid_request')
    run_cli(twin.cli_db, '--base-lineage', '0192f000-0000-7000-8000-0000000000c2', 'run', 'record', RUN,
            'heartbeat', '--sequence', '9', '--actor', WORKER, error='lineage_mismatch')
    twin.agent.call('record_run_activity', {'run': RUN, 'source_sequence': 9, 'kind': 'heartbeat', 'base_lineage': '0192f000-0000-7000-8000-0000000000c2'},
                    error='lineage_mismatch')
    twin.agent.call('record_run_activity', {'run': RUN, 'source_sequence': 9, 'kind': 'heartbeat', 'base_revision': 3},
                    error='invalid_request')


def recovery_policy(directory, twin):
    """A backup carries the run store and says so; a restore forks it and labels older runs foreign."""
    backup = directory / 'runs-backup.sqlite'
    report = run_cli(twin.cli_db, 'backup', '--to', str(backup))
    assert report['runs']['runs'] == 1 and report['runs']['archived'] is True and report['runs']['lifecycle_events'] == 3 and report['runs']['activity_retained'] == 3, report
    assert (directory / 'runs-backup.sqlite.runs').exists()
    assert run_cli(twin.cli_db, 'verify-store')['runs']['runs'] == 1
    assert run_cli(backup, 'verify-store')['runs']['archived'] is True
    # A backup archive is never a live workspace: no run write opens, creates or replays anything,
    # and reads stay available.
    sidecar = directory / 'runs-backup.sqlite.runs'
    before = sidecar.read_bytes()
    archived_operation = run_cli(backup, 'history')['entries'][0]['operation']['id']
    for refused in [
        ('run', 'start', 'TEST-A', '--run-id', RUN, '--actor', WORKER),
        ('run', 'report', RUN, 'failed', '--actor', WORKER),
        ('run', 'record', RUN, 'heartbeat', '--sequence', '1', '--actor', WORKER),
        ('run', 'link', RUN, archived_operation, '--actor', WORKER),
    ]:
        run_cli(backup, *refused, error='archived_store')
    assert sidecar.read_bytes() == before, 'the run archive was written'
    assert run_cli(backup, 'run', 'show', RUN)['state'] == 'completed'
    restored = directory / 'runs-restored.sqlite'
    outcome = run_cli(directory / 'unused.sqlite', 'restore', '--from', str(backup), '--to', str(restored))
    assert outcome['runs']['archived'] is False and outcome['runs']['lineage_id'] == outcome['lineage_id'], outcome
    view = run_cli(restored, 'run', 'show', RUN)
    original = run_cli(twin.cli_db, 'revision')['lineage_id']
    assert view['lineage'] == 'foreign' and view['run']['contract']['lineage_id'] == original, view
    assert view['status'] == 'completed', 'a finished run keeps its recorded end'
    run_cli(restored, 'run', 'show', '0192f000-0000-7000-8000-0000000000ee', error='not_found')
    # A run from before the restore is a historical observation: no new fact of any kind is
    # recorded for it, retries included, and the error names the lineage it was recorded under.
    operation = run_cli(restored, 'history')['entries'][0]['operation']['id']
    for refused in [
        ('run', 'report', RUN, 'failed', '--actor', WORKER),
        ('run', 'record', RUN, 'heartbeat', '--sequence', '99', '--actor', WORKER),
        ('run', 'link', RUN, operation, '--actor', WORKER),
        ('run', 'start', 'TEST-A', '--run-id', RUN, '--actor', WORKER),
    ]:
        error = run_cli(restored, *refused, error='foreign_run')['error']
        assert error['details']['recorded'] == original and error['details']['run'] == RUN, error
    assert run_cli(restored, 'run', 'lifecycle') == run_cli(restored, 'run', 'lifecycle')
    assert run_cli(restored, 'run', 'show', RUN)['state'] == 'completed'
    assert run_cli(restored, 'run', 'activity')['head_sequence'] == 3
    # A store with no runs backs up without a run store and its report does not mention one.
    plain = directory / 'plain.sqlite'
    run_cli(plain, 'import', str(FIXTURE))
    plain_backup = directory / 'plain-backup.sqlite'
    assert 'runs' not in run_cli(plain, 'backup', '--to', str(plain_backup))
    assert not (directory / 'plain-backup.sqlite.runs').exists()
    # A run-free archive takes no run write either, and none creates a run store beside it.
    run_cli(plain_backup, 'run', 'start', 'TEST-A', '--actor', WORKER, error='archived_store')
    assert not (directory / 'plain-backup.sqlite.runs').exists()
    assert run_cli(plain_backup, 'run', 'list') == {'runs': []}
    # Reading runs never creates a run store.
    assert run_cli(plain, 'run', 'list') == {'runs': []}
    assert not Path(str(plain) + '.runs').exists()


def smoke(directory):
    twin = Twin(directory)
    try:
        source_parity(twin, directory)
        before = run_cli(twin.cli_db, 'export')
        lifecycle_parity(twin)
        activity_parity(twin)
        freshness_parity(twin)
        project_unchanged(twin, 3, 3)
        assert run_cli(twin.cli_db, 'export') == before, 'run commands changed the plan'
        link_and_finish_parity(twin)
        report_parity(twin)
        read_parity(twin)
        preconditions(twin)
        project_unchanged_after = run_cli(twin.cli_db, 'revision')['revision']
        assert project_unchanged_after == 5, 'only the project commands advanced the revision'
        recovery_policy(directory, twin)
    finally:
        twin.close()
    print('PASS: CLI/MCP run start, retries, conflicts, activity, transitions, links and feeds; project untouched')
    print('PASS: stale telemetry on the pinned clock, bounded activity text, and backup, restore and verification of runs')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-runs-') as directory:
        smoke(Path(directory))
