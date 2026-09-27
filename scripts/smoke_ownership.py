#!/usr/bin/env python3
"""Release and handoff behave identically through the CLI and MCP adapters and stay in history."""
import json
import tempfile
import uuid
from pathlib import Path

from smoke_agent import ROOT, Agent, run_cli


def revision(database):
    return run_cli(database, 'export')['revision']


def same_refusal(agent, tool, arguments, database, command, code='invalid_command'):
    """Both adapters refuse alike with the same message, and nothing changes."""
    before = run_cli(database, 'export')
    remote = agent.call(tool, {'base_revision': before['revision'], **arguments}, error=code)
    local = run_cli(database, *command, error=code)['error']
    assert remote['message'] == local['message'], (remote, local)
    assert run_cli(database, 'export') == before
    return local['message']


def evidence(directory, author):
    """An artifact attributed to `author`, as the MCP argument and as a CLI input file."""
    kind, name = author.split(':')
    artifact = {'id': str(uuid.uuid4()), 'kind': 'TestResult', 'uri': 'file:report.txt', 'label': 'Report',
                'metadata': {}, 'created_by': {'kind': kind.capitalize(), 'name': name},
                'created_at': '2026-09-01T00:00:00Z'}
    path = directory / f"{artifact['id']}.json"
    path.write_text(json.dumps(artifact))
    return artifact, str(path)


def ownership_smoke(directory):
    database = directory / 'ownership.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    lead = Agent(database, 'human:lead')
    try:
        artifact, path = evidence(directory, 'agent:parity')
        message = same_refusal(worker, 'add_artifact', {'key': 'TEST-A', 'artifact': artifact}, database,
                               ('artifact', 'TEST-A', path, '--actor', 'agent:parity'))
        assert 'unclaimed task' in message, message
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})
        worker.call('add_artifact', {'key': 'TEST-A', 'artifact': artifact, 'base_revision': 1})
        run_cli(database, 'release', 'TEST-A', '--reason', 'evidence first', '--actor', 'agent:parity')
        release = ('release', 'TEST-A', '--reason', 'wrong task')
        message = same_refusal(worker, 'release_work', {'key': 'TEST-A', 'reason': 'wrong task'}, database,
                               (*release, '--actor', 'agent:parity'))
        assert 'Planned' in message, message
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 3})
        same_refusal(lead, 'release_work', {'key': 'TEST-A', 'reason': 'wrong task'}, database,
                     (*release, '--actor', 'human:lead'))
        released = worker.call('release_work', {'key': 'TEST-A', 'reason': 'wrong task', 'base_revision': 4})['data']
        work = released['command']['Release']['work']
        assert released['command'] == {'Release': {'work': work, 'reason': 'wrong task'}}
        shown = worker.call('get_work', {'key': 'TEST-A'})['data']
        assert shown == run_cli(database, 'show', 'TEST-A')
        assert shown['execution']['status'] == 'Planned' and shown['execution']['owner'] is None and 'handoffs' not in shown
        assert [(r['actor']['name'], r['reason']) for r in shown['execution']['releases']] == [
            ('parity', 'evidence first'), ('parity', 'wrong task')], shown['execution']['releases']
        run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:parity')
        local = run_cli(database, *release, '--actor', 'agent:parity')
        assert local['command'] == released['command'] and local['actor'] == released['actor']
        run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:parity')
        run_cli(database, 'start', 'TEST-A', '--actor', 'agent:parity')
        message = same_refusal(worker, 'release_work', {'key': 'TEST-A', 'reason': 'stuck'}, database,
                               ('release', 'TEST-A', '--reason', 'stuck', '--actor', 'agent:parity'))
        assert 'has started' in message, message
        handoff = ('handoff', 'TEST-A', '--to', 'agent:second', '--reason', 'parity agent stopped')
        arguments = {'key': 'TEST-A', 'to': 'agent:second', 'reason': 'parity agent stopped'}
        message = same_refusal(worker, 'handoff_work', arguments, database, (*handoff, '--actor', 'agent:parity'))
        assert 'authorize a handoff' in message, message
        same_refusal(lead, 'handoff_work', {**arguments, 'to': 'second'}, database,
                     ('handoff', 'TEST-A', '--to', 'second', '--reason', 'r', '--actor', 'human:lead'), 'invalid_request')
        same_refusal(lead, 'handoff_work', {**arguments, 'key': 'missing'}, database,
                     ('handoff', 'missing', '--to', 'agent:second', '--reason', 'r', '--actor', 'human:lead'), 'not_found')
        lead.call('handoff_work', {'key': 'TEST-A', 'base_revision': 5}, error='invalid_request')
        before = worker.call('get_work', {'key': 'TEST-A'})['data']
        moved = lead.call('handoff_work', {**arguments, 'base_revision': revision(database)})['data']
        assert moved['command'] == {'Handoff': {
            'work': work, 'from': {'kind': 'Agent', 'name': 'parity'}, 'to': {'kind': 'Agent', 'name': 'second'},
            'reason': 'parity agent stopped'}}
        after = worker.call('get_work', {'key': 'TEST-A'})['data']
        assert after == run_cli(database, 'show', 'TEST-A')
        assert after['execution']['owner'] == {'kind': 'Agent', 'name': 'second'} and after['execution']['events'] == before['execution']['events']
        assert after['execution']['status'] == 'InProgress' and after['execution']['handoffs'][0]['actor'] == {'kind': 'Human', 'name': 'lead'}
        run_cli(database, *handoff[:2], '--to', 'agent:parity', '--reason', 'back', '--actor', 'human:lead')
        run_cli(database, *handoff, '--actor', 'human:lead')
        run_cli(database, 'submit', 'TEST-A', '--actor', 'agent:second')
        message = same_refusal(worker, 'verify_work', {'key': 'TEST-A'}, database,
                               ('verify', 'TEST-A', '--actor', 'agent:parity'))
        assert 'held before a handoff' in message, message
        same_refusal(lead, 'handoff_work', arguments, database, (*handoff, '--actor', 'human:lead'))
        # A past independent rejection stays valid when its reviewer takes over the rework.
        lead.call('reject_work', {'key': 'TEST-A', 'reason': 'report missing', 'base_revision': revision(database)})
        takeover = {'key': 'TEST-A', 'to': 'human:lead', 'reason': 'reviewer reworks it'}
        lead.call('handoff_work', {**takeover, 'base_revision': revision(database)})
        run_cli(database, 'submit', 'TEST-A', '--actor', 'human:lead')
        for name in ('agent:parity', 'agent:second'):
            former = worker if name == 'agent:parity' else Agent(database, name)
            try:
                message = same_refusal(former, 'verify_work', {'key': 'TEST-A'}, database,
                                       ('verify', 'TEST-A', '--actor', name))
                assert 'held before a handoff or release' in message, message
            finally:
                if former is not worker:
                    former.close()
        run_cli(database, 'verify', 'TEST-A', '--actor', 'human:reviewer')
        shown = run_cli(database, 'show', 'TEST-A')
    finally:
        worker.close()
        lead.close()
    # Ownership records and history survive a restart of both adapters.
    worker = Agent(database, 'agent:parity')
    try:
        assert worker.call('get_work', {'key': 'TEST-A'})['data'] == shown == run_cli(database, 'show', 'TEST-A')
        assert len(shown['execution']['releases']) == 3 and len(shown['execution']['handoffs']) == 4, shown
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        kinds = [next(iter(e['operation']['command'])) for e in history['entries']]
        assert kinds.count('Release') == 3 and kinds.count('Handoff') == 4, kinds
    finally:
        worker.close()
    independent_evidence(directory)


def independent_evidence(directory):
    """Evidence authors and releasers never review, whoever imported or holds the task."""
    database = directory / 'evidence.sqlite'
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    artifact, _ = evidence(directory, 'human:auditor')
    plan['artifacts'][artifact['id']] = artifact
    work = next(w for w in plan['work_items'].values() if w['key'] == 'TEST-A')
    work['execution']['artifact_ids'] = [artifact['id']]
    imported = directory / 'evidence-plan.json'
    imported.write_text(json.dumps(plan))
    run_cli(database, 'import', str(imported))
    run_cli(database, 'claim', 'TEST-A', '--actor', 'human:early')
    run_cli(database, 'release', 'TEST-A', '--reason', 'not mine', '--actor', 'human:early')
    for step in ('claim', 'start', 'submit'):
        run_cli(database, step, 'TEST-A', '--actor', 'agent:parity')
    auditor = Agent(database, 'human:auditor')
    early = Agent(database, 'human:early')
    try:
        for tool, command in (('verify_work', ('verify',)), ('reject_work', ('reject', 'r'))):
            extra = {'reason': 'r'} if tool == 'reject_work' else {}
            message = same_refusal(auditor, tool, {'key': 'TEST-A', **extra}, database,
                                   (command[0], 'TEST-A', *command[1:], '--actor', 'human:auditor'))
            assert 'evidence it authored' in message, message
        message = same_refusal(early, 'verify_work', {'key': 'TEST-A'}, database,
                               ('verify', 'TEST-A', '--actor', 'human:early'))
        assert 'handoff or release' in message, message
        early.call('reject_work', {'key': 'TEST-A', 'reason': 'r', 'base_revision': revision(database)},
                   error='invalid_command')
    finally:
        auditor.close()
        early.close()
    run_cli(database, 'verify', 'TEST-A', '--actor', 'human:lead')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-ownership-') as directory:
        ownership_smoke(Path(directory))
    print('PASS: CLI/MCP release and handoff parity, refusals, evidence-author and holder independence, history after restart')
