#!/usr/bin/env python3
"""Import/query the prepared self-host example; never execute its task contracts."""
import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from smoke_agent import Agent, ROOT, run_cli

SEED = ROOT / 'examples/self-host/dpm-alpha.json'
EXPECTED = ROOT / 'examples/self-host/dpm-alpha.expected.json'


def audit_contracts(database, actor, plan, expected):
    tasks = [w for w in plan['work_items'].values() if w['kind'] == 'Task']
    assert len(tasks) == expected['task_count']
    for task in tasks:
        key = task['key']
        contract = task['instructions']
        assert len(contract['steps']) >= 3, key
        assert all(step['action'].strip() and step['expected_result'].strip() for step in contract['steps']), key
        for field in ('in_scope', 'out_of_scope', 'verification'):
            assert contract[field] and all(text.strip() for text in contract[field]), (key, field)
        assert len(task['acceptance']) >= 3, key
        shown = run_cli(database, 'show', key)
        tool = actor.call('get_work', {'key': key})
        assert tool['revision'] == 0 and tool['data'] == shown, key
        for field, value in task.items():
            assert shown[field] == value, (key, field)
        detail = run_cli(database, 'explain', key)
        tool = actor.call('explain_work', {'key': key})
        assert tool['revision'] == 0 and tool['data'] == detail, key
        assert detail['work'] == task and not detail['ready'], key
        context = detail['context']
        assert context['requirements'] and context['artifacts'] and context['risks'], key
        assert expected['execution_gate'] in {gate['key'] for gate in context['decisions']}, key
        choices = expected['context_decisions']
        actual_choices = {d['key']: d for d in context['decisions'] if d['key'] in choices}
        assert set(actual_choices) == {choice for choice, tasks in choices.items() if key in tasks}, key
        source_ids = {artifact['id'] for artifact in context['artifacts']}
        for decision in actual_choices.values():
            assert decision['status'] == 'Decided' and not decision['blocks'], key
            assert decision['rationale'].strip() and decision['artifact_ids'], key
            assert set(decision['artifact_ids']) <= source_ids, key
        edges = [d for d in plan['dependencies'] if task['id'] in (d['predecessor'], d['successor'])]
        assert context['dependencies'] == edges, key
    print(f"PASS: all {len(tasks)} task contracts expose ordered steps, scope, acceptance and resolved context identically through CLI/MCP", flush=True)


def smoke(directory):
    launch = subprocess.run(
        ['cargo', 'run', '--quiet', '--locked', 'tui', '--help'],
        cwd=ROOT, capture_output=True, text=True, timeout=120,
    )
    assert launch.returncode == 0 and 'dpm tui' in launch.stdout, (launch.stdout, launch.stderr)
    original = SEED.read_bytes()
    plan = json.loads(original)
    expected = json.loads(EXPECTED.read_text())
    database = directory / 'self-host.sqlite'
    assert run_cli(database, 'validate', str(SEED))['valid']
    initialized = run_cli(database, 'demo')
    assert initialized['workspace'] == expected['workspace']
    assert initialized['revision'] == expected['revision']
    assert run_cli(database, 'export') == plan
    actor = Agent(database, 'agent:preview-reader')
    try:
        pairs = [
            ('project_status', {'probabilistic': False}, ('status', '--no-simulation')),
            ('next_work', {'probabilistic': False}, ('next', '--deterministic-only')),
            ('get_work', {'key': 'M0-MVP'}, ('show', 'M0-MVP')),
            ('explain_work', {'key': expected['first_contract']}, ('explain', expected['first_contract'])),
            ('explain_work', {'key': 'SERVER-10'}, ('explain', 'SERVER-10')),
        ]
        for tool, arguments, command in pairs:
            result = actor.call(tool, arguments)
            assert result['revision'] == expected['revision']
            assert result['data'] == run_cli(database, *command)
        audit_contracts(database, actor, plan, expected)
        summary = run_cli(database, 'status', '--no-simulation')
        assert summary['total_work'] == expected['total_work']
        assert summary['ready'] == summary['in_flight'] == summary['awaiting_verification'] == 0
        assert summary['complete'] == expected['completed_work']
        assert summary['open_decisions'] == expected['open_decisions']
        assert summary['progress'] == {'percent_complete': 0.0, 'verified': False}
        assert run_cli(database, 'next', '--deterministic-only') == expected['ready_keys']
        detail = run_cli(database, 'explain', expected['first_contract'])
        assert not detail['ready'] and len(detail['work']['acceptance']) >= 3
        context = detail['context']
        assert context['requirements'] and context['artifacts'] and context['risks']
        assert expected['execution_gate'] in {gate['key'] for gate in context['decisions']}
        assert any(expected['execution_gate'] in why for why in detail['why_now'])
        future = run_cli(database, 'explain', 'SERVER-10')
        assert {'DEC-EXECUTE', 'DEC-EXPAND', 'DEC-POST-MVP'} <= {gate['key'] for gate in future['context']['decisions']}
        for artifact in plan['artifacts'].values():
            if artifact['uri'].startswith('repo:'):
                path = (ROOT / artifact['uri'][5:]).resolve()
                assert path.is_relative_to(ROOT) and path.is_file(), path
        run_cli(database, 'demo', error='storage_error')
        assert run_cli(database, 'export') == plan
        with sqlite3.connect(database) as connection:
            assert connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0] == expected['operations']
            snapshot = json.loads(connection.execute('SELECT plan_json FROM plan_state').fetchone()[0])
            assert snapshot == plan and snapshot['revision'] == 0
        assert SEED.read_bytes() == original
    finally:
        actor.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-self-host-readonly-') as temporary:
        smoke(Path(temporary))
    print('PASS: self-host demo import, read-only CLI/MCP parity, open scope gates, revision 0 and zero operations')
