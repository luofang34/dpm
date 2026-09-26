#!/usr/bin/env python3
"""Provisional submission bases read and change identically through the CLI and MCP adapters."""
import json
import tempfile
from pathlib import Path

from smoke_agent import ROOT, Agent, run_cli


def provisional_plan():
    """TEST-A -FS-> TEST-B with a provisional start basis and no decision gates."""
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    keys = {w['key']: w['id'] for w in plan['work_items'].values()}
    plan['decisions'] = {}
    [edge] = [d for d in plan['dependencies'] if d['predecessor'] == keys['TEST-A'] and d['successor'] == keys['TEST-B']]
    edge['start_basis'] = 'Provisional'
    return plan


def same_views(worker, database):
    """Every query view is the same JSON through both adapters; returns B's explanation."""
    detail = worker.call('explain_work', {'key': 'TEST-B'})['data']
    assert detail == run_cli(database, 'explain', 'TEST-B')
    assert worker.call('explain_work', {'key': 'TEST-A'})['data'] == run_cli(database, 'explain', 'TEST-A')
    assert worker.call('project_status', {'probabilistic': False})['data'] == run_cli(database, 'status', '--no-simulation')
    assert worker.call('next_work', {'probabilistic': False})['data'] == run_cli(database, 'next', '--deterministic-only')
    return detail


def flagged(detail):
    gates = [g for g in detail['transitions']['verify']['unmet'] if g['type'] == 'basis_invalidated']
    assert gates == [g for g in detail['transitions']['submit']['unmet'] if g['type'] == 'basis_invalidated']
    return gates


def provisional_smoke(directory):
    fixture = directory / 'provisional.json'
    fixture.write_text(json.dumps(provisional_plan()))
    database = directory / 'provisional.sqlite'
    run_cli(database, 'import', str(fixture))
    author = Agent(database, 'agent:author')
    builder = Agent(database, 'agent:builder')
    reviewer = Agent(database, 'human:reviewer')
    try:
        for revision, tool in enumerate(['claim_work', 'start_work', 'submit_work']):
            author.call(tool, {'key': 'TEST-A', 'base_revision': revision})
        claimable = same_views(builder, database)
        assert claimable['ready'] and [r['attempt'] for r in claimable['gates']['provisional']] == [1]
        reliance = 'provisional: TEST-A attempt #1 is only submitted'
        assert any(reliance in reason for reason in claimable['why_now'])
        [candidate] = [c for c in builder.call('next_work', {'probabilistic': False})['data']['candidates'] if c['work']['key'] == 'TEST-B']
        assert any(reliance in reason for reason in candidate['reasons'])
        builder.call('claim_work', {'key': 'TEST-B', 'base_revision': 3})
        run_cli(database, 'start', 'TEST-B', '--actor', 'agent:builder')
        started = same_views(builder, database)
        assert [(b['attempt'], b['source']['kind']) for b in started['work']['basis']] == [(1, 'start')]
        verify_gate = [g for g in started['transitions']['verify']['unmet'] if g['type'] == 'dependency']
        assert verify_gate[0]['start_basis'] == 'Provisional' and 'accepts_submission' not in verify_gate[0]
        reviewer.call('reject_work', {'key': 'TEST-A', 'reason': 'fails acceptance', 'base_revision': 5})
        rejected = same_views(builder, database)
        [gate] = flagged(rejected)
        assert (gate['attempt'], gate['current_attempt'], gate['state']['state']) == (1, None, 'invalidated')
        assert run_cli(database, 'status', '--no-simulation')['basis_invalidated'] == 1
        edge = gate['dependency']
        refusals = [
            (builder, {'attempt': 1, 'reason': 'ok'}, ('--actor', 'agent:builder'), 'invalid_command'),
            (reviewer, {'attempt': 1, 'reason': 'ok'}, (), 'invalid_command'),
            (reviewer, {'attempt': 1, 'reason': ' '}, (), 'invalid_command'),
            (reviewer, {'attempt': 1, 'reason': 'ok', 'base_revision': 0}, ('--base-revision', '0'), 'revision_conflict'),
        ]
        for agent, arguments, extra, code in refusals:
            arguments = {'key': 'TEST-B', 'dependency': edge, 'base_revision': 6, **arguments}
            remote = agent.call('revalidate_basis', arguments, error=code)
            command = ['revalidate-basis', 'TEST-B', '--dependency', edge, '--attempt', str(arguments['attempt']), '--reason', arguments['reason']]
            if extra[:1] == ('--base-revision',):
                command = [*extra, *command]
            else:
                command = [*command, *extra]
            assert remote['message'] == run_cli(database, *command, error=code)['error']['message'], arguments
        assert same_views(builder, database) == rejected, 'refused revalidations change nothing'
        author.call('submit_work', {'key': 'TEST-A', 'base_revision': 6})
        run_cli(database, 'verify', 'TEST-A', '--actor', 'human:reviewer')
        verified = same_views(builder, database)
        [gate] = flagged(verified)
        assert (gate['attempt'], gate['current_attempt']) == (1, 2), 'a later verification never validates B'
        stale = reviewer.call('revalidate_basis', {'key': 'TEST-B', 'dependency': edge, 'attempt': 1, 'reason': 'ok', 'base_revision': 8}, error='invalid_command')
        assert 'current unrejected attempt is #2' in stale['message']
        done = reviewer.call('revalidate_basis', {'key': 'TEST-B', 'dependency': edge, 'attempt': 2, 'reason': 'B matches A2', 'base_revision': 8})['data']
        assert done['command'] == {'RevalidateBasis': {'work': started['work']['id'], 'dependency': edge, 'attempt': 2, 'reason': 'B matches A2'}}
        revalidated = same_views(builder, database)
        assert flagged(revalidated) == [] and revalidated['transitions']['submit']['ready']
        [status] = revalidated['basis']['relies_on']
        assert (status['basis']['attempt'], status['state']['state']) == (2, 'verified')
        history = builder.call('history', {'after_sequence': 0})['data']
        assert history == run_cli(database, 'history', '--after-sequence', '0')
        assert history['entries'][-1]['operation']['actor'] == {'kind': 'Human', 'name': 'reviewer'}
    finally:
        for agent in (author, builder, reviewer):
            agent.close()
    print('PASS: CLI/MCP provisional starts, rejected-attempt invalidation, stale/unauthorized revalidation and reviewed re-basing')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-provisional-') as directory:
        provisional_smoke(Path(directory))
