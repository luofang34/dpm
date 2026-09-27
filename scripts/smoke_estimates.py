#!/usr/bin/env python3
"""Unestimated work is named identically by the CLI and MCP adapters wherever a forecast appears."""
import json
import subprocess
import tempfile
from pathlib import Path

from smoke_agent import CLI, ROOT, Agent, run_cli


def unestimated_plan():
    """The execution fixture with TEST-A and TEST-E missing their three-point estimates."""
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    for work in plan['work_items'].values():
        if work['key'] in ('TEST-A', 'TEST-E'):
            work['schedule']['estimate'] = None
    return plan


def same_status(agent, database):
    for arguments, extra in (({}, ()), ({'probabilistic': False}, ('--no-simulation',))):
        remote = agent.call('project_status', arguments)['data']
        assert remote == run_cli(database, 'status', *extra), arguments
    return remote


def estimates_smoke(directory):
    estimated = directory / 'estimated.sqlite'
    run_cli(estimated, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    agent = Agent(estimated, 'agent:estimator')
    try:
        assert same_status(agent, estimated)['unestimated'] == []
        assert agent.call('explain_work', {'key': 'TEST-A'})['data']['unestimated'] is False
    finally:
        agent.close()

    fixture = directory / 'unestimated.json'
    fixture.write_text(json.dumps(unestimated_plan()))
    database = directory / 'unestimated.sqlite'
    run_cli(database, 'import', str(fixture))
    agent = Agent(database, 'agent:estimator')
    try:
        assert same_status(agent, database)['unestimated'] == ['TEST-A', 'TEST-E']
        for key, expected in (('TEST-A', True), ('TEST-B', False), ('TEST-M1', False)):
            explained = agent.call('explain_work', {'key': key})['data']
            assert explained == run_cli(database, 'explain', key), key
            assert explained['unestimated'] is expected, key
            assert any('0 h' in reason for reason in explained['why_now']) is expected, key
        ranked = agent.call('next_work', {})['data']
        assert ranked == run_cli(database, 'next')
        [first] = [c for c in ranked['candidates'] if c['work']['key'] == 'TEST-A']
        assert any('0 h' in reason for reason in first['reasons']), first

        text = subprocess.run([str(CLI), '--database', str(database), 'status'], cwd=ROOT,
                              capture_output=True, text=True, timeout=15, check=True).stdout
        assert text.splitlines()[0] == 'Unestimated: 2 task(s) count as 0 h, so the forecast is optimistic: TEST-A, TEST-E', text

        # Submitted work keeps its duration until verification; only verification removes it.
        for command in ('claim', 'start', 'submit'):
            run_cli(database, command, 'TEST-A')
        assert same_status(agent, database)['unestimated'] == ['TEST-A', 'TEST-E']
        run_cli(database, 'verify', 'TEST-A')
        assert same_status(agent, database)['unestimated'] == ['TEST-E']
        assert agent.call('explain_work', {'key': 'TEST-A'})['data']['unestimated'] is False
    finally:
        agent.close()
    check_scenarios(directory)
    print('PASS: CLI/MCP unestimated work in status, explain, next and open-choice scenarios; text notice; verification removes it')


def check_scenarios(directory):
    """Undecided branches are named only by the scenarios that select them, identically in both adapters."""
    plan = json.loads((ROOT / 'tests/support/conditional-plan.json').read_text())
    for work in plan['work_items'].values():
        if work['key'] in ('SUP-DESIGN', 'SUP-A-QUOTE', 'SUP-B-QUOTE'):
            work['schedule']['estimate'] = None
    fixture, database = directory / 'estimates-conditional.json', directory / 'estimates-conditional.sqlite'
    fixture.write_text(json.dumps(plan))
    run_cli(database, 'import', str(fixture))
    agent = Agent(database, 'agent:estimator')
    try:
        status = same_status(agent, database)
        assert status['unestimated'] == ['SUP-DESIGN'], status['unestimated']
        listed = {tuple(sorted(s['unestimated'])) for s in status['open_choices']['scenarios']}
        assert ('SUP-A-QUOTE', 'SUP-DESIGN') in listed and ('SUP-B-QUOTE', 'SUP-DESIGN') in listed, listed
    finally:
        agent.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-estimates-') as directory:
        estimates_smoke(Path(directory))
