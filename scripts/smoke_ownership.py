#!/usr/bin/env python3
"""Release and handoff behave identically through the CLI and MCP adapters and stay in history."""
import tempfile
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


def ownership_smoke(directory):
    database = directory / 'ownership.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    lead = Agent(database, 'human:lead')
    try:
        release = ('release', 'TEST-A', '--reason', 'wrong task')
        message = same_refusal(worker, 'release_work', {'key': 'TEST-A', 'reason': 'wrong task'}, database,
                               (*release, '--actor', 'agent:parity'))
        assert 'Planned' in message, message
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})
        same_refusal(lead, 'release_work', {'key': 'TEST-A', 'reason': 'wrong task'}, database,
                     (*release, '--actor', 'human:lead'))
        released = worker.call('release_work', {'key': 'TEST-A', 'reason': 'wrong task', 'base_revision': 1})['data']
        work = released['command']['Release']['work']
        assert released['command'] == {'Release': {'work': work, 'reason': 'wrong task'}}
        shown = worker.call('get_work', {'key': 'TEST-A'})['data']
        assert shown == run_cli(database, 'show', 'TEST-A')
        assert shown['status'] == 'Planned' and shown['owner'] is None and 'handoffs' not in shown
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
        assert after['owner'] == {'kind': 'Agent', 'name': 'second'} and after['events'] == before['events']
        assert after['status'] == 'InProgress' and after['handoffs'][0]['actor'] == {'kind': 'Human', 'name': 'lead'}
        run_cli(database, *handoff[:2], '--to', 'agent:parity', '--reason', 'back', '--actor', 'human:lead')
        run_cli(database, *handoff, '--actor', 'human:lead')
        run_cli(database, 'submit', 'TEST-A', '--actor', 'agent:second')
        message = same_refusal(worker, 'verify_work', {'key': 'TEST-A'}, database,
                               ('verify', 'TEST-A', '--actor', 'agent:parity'))
        assert 'held before a handoff' in message, message
        same_refusal(lead, 'handoff_work', arguments, database, (*handoff, '--actor', 'human:lead'))
        lead.call('verify_work', {'key': 'TEST-A', 'base_revision': revision(database)})
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        kinds = [next(iter(e['operation']['command'])) for e in history['entries']]
        assert kinds.count('Release') == 2 and kinds.count('Handoff') == 3, kinds
    finally:
        worker.close()
        lead.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-ownership-') as directory:
        ownership_smoke(Path(directory))
    print('PASS: CLI/MCP release and handoff parity, refusals, independent review and history')
