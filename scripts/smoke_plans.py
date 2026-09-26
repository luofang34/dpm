#!/usr/bin/env python3
"""Check reviewed planning and durable history through the real CLI and agent transport."""
import copy
import json
import sqlite3
import tempfile
from pathlib import Path

from smoke_agent import Agent, ROOT, run_cli


def smoke(directory):
    database = directory / 'planning.sqlite'
    candidate = directory / 'proposal.json'
    run_cli(database, 'init', 'Editable workspace')
    worker = Agent(database, 'agent:planner')
    reviewer = Agent(database, 'human:reviewer')
    try:
        initial = run_cli(database, 'export')
        assert worker.call('export_plan', {})['data'] == initial
        proposal = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
        proposal['workspace'] = initial['workspace']
        for work in proposal['work_items'].values():
            if work['kind'] == 'Task':
                work['status'] = 'Proposed'
        candidate.write_text(json.dumps(proposal))
        preview = worker.call('propose_change', {'plan': proposal})['data']
        assert preview == run_cli(database, 'plan', 'diff', str(candidate))
        assert preview['base_revision'] == 0 and preview['changes']
        assert run_cli(database, 'export') == initial
        refused = worker.call('apply_change', {'plan': proposal, 'reason': 'self-approve', 'base_revision': 0}, error='invalid_command')
        assert refused['message'] == run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'self-approve', '--actor', 'agent:planner', error='invalid_command')['error']['message']
        approved = run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Prepare contracts for review', '--actor', 'human:reviewer')
        assert approved['resulting_revision'] == 1
        assert worker.call('next_work', {})['data'] == []
        reviewer.call('ratify_contract', {'key': 'TEST-A', 'base_revision': 1})
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 2})
        changed = worker.call('export_plan', {})['data']
        next(w for w in changed['work_items'].values() if w['key'] == 'TEST-F')['title'] = 'Future integration contract'
        candidate.write_text(json.dumps(changed))
        diff = worker.call('propose_change', {'plan': changed})['data']
        assert diff == run_cli(database, 'plan', 'diff', str(candidate))
        assert diff['changes'][0]['fields'] == ['title']
        reviewer.call('apply_change', {'plan': changed, 'reason': 'Clarify future integration', 'base_revision': 3})
        current = run_cli(database, 'export')
        assert current['revision'] == 4
        stale = worker.call('propose_change', {'plan': changed}, error='revision_conflict')
        assert stale['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='revision_conflict')['error']['message']
        reject_invalid(database, worker, reviewer, candidate, current)
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        assert len(history['entries']) == 4 and history['revision'] == 4
        assert history['entries'][0]['operation'] == approved
        assert history['entries'][-1]['operation']['command']['ApplyChange']['reason'] == 'Clarify future integration'
        for after in [0, 2, 4, 2**64-1]:
            assert worker.call('history', {'after_sequence': after, 'limit': 2})['data'] == run_cli(database, 'history', '--after-sequence', str(after), '--limit', '2')
        with sqlite3.connect(database) as connection:
            assert connection.execute('SELECT COUNT(*) FROM operations').fetchone()[0] == 4
    finally:
        worker.close()
        reviewer.close()
    reopened = Agent(database, 'agent:reader')
    try:
        assert reopened.call('export_plan', {})['data'] == current
        assert reopened.call('history', {})['data'] == history
    finally:
        reopened.close()


def reject_invalid(database, worker, reviewer, candidate, current):
    for error_case in ['acceptance', 'cycle', 'lifecycle', 'gate', 'noop']:
        bad = copy.deepcopy(current)
        task = next(w for w in bad['work_items'].values() if w['key'] == 'TEST-A')
        if error_case == 'acceptance':
            task['acceptance'] = [{'text': 'Lower the accepted result'}]
        elif error_case == 'cycle':
            edge = bad['dependencies'][0]
            bad['dependencies'].append({**edge, 'predecessor': edge['successor'], 'successor': edge['predecessor']})
        elif error_case == 'lifecycle':
            task['status'] = 'Verified'
        elif error_case == 'gate':
            gate = next(iter(bad['decisions'].values()))
            gate['status'], gate['outcome'] = 'Decided', 'Bypass'
        candidate.write_text(json.dumps(bad))
        if error_case != 'noop':
            remote = worker.call('propose_change', {'plan': bad}, error='invalid_command')
            assert remote['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='invalid_command')['error']['message']
        remote = reviewer.call('apply_change', {'plan': bad, 'reason': 'Invalid proposal', 'base_revision': 4}, error='invalid_command')
        assert remote['message'] == run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Invalid proposal', error='invalid_command')['error']['message']
        assert run_cli(database, 'export') == current


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-plan-') as temporary:
        smoke(Path(temporary))
    print('PASS: empty workspace to reviewed graph, CLI/MCP plan diff/apply/history parity, protected execution, stale proposals and durable restart')
