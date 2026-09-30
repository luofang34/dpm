#!/usr/bin/env python3
"""Start, submit and verify record when their event occurred beside the operation's commit time,
and refuse an occurrence time after the commit or before the task's previous event, alike through
the CLI and MCP."""
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from smoke_agent import ROOT, Agent, run_cli

FIXTURE = ROOT / 'tests/support/execution-plan.json'


def occurrence_smoke(directory):
    database = directory / 'occurrence.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    now = datetime.now(timezone.utc).replace(microsecond=0)

    def hours(offset):
        return (now + timedelta(hours=offset)).strftime('%Y-%m-%dT%H:%M:%SZ')

    run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:worker')
    worker = Agent(database, 'agent:worker')
    reviewer = Agent(database, 'human:reviewer')
    try:
        started = worker.call('start_work', {'key': 'TEST-A', 'base_revision': 1, 'at': hours(-3)})['data']
        assert started['command']['Start']['occurred_at'] == hours(-3) and started['timestamp'] > hours(-3), started
        assert worker.call('get_work', {'key': 'TEST-A'})['data']['execution']['events'] == {'started_at': hours(-3)}
        before = run_cli(database, 'export')
        refusals = [
            (hours(-4), 'invalid_command', 'before its previous recorded event'),
            (hours(1), 'invalid_command', 'after the operation commits'),
            ('yesterday', 'invalid_request', ''),
        ]
        for at, code, reason in refusals:
            remote = worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 2, 'at': at}, error=code)
            local = run_cli(database, 'submit', 'TEST-A', '--at', at, '--actor', 'agent:worker', error=code)['error']
            assert reason in remote['message'] and reason in local['message'], (remote, local)
            if code == 'invalid_command' and reason.startswith('before'):
                # Both times are the caller's and the record's, so the whole refusal matches.
                assert remote == local and 'TEST-A' in local['message'], (remote, local)
        assert run_cli(database, 'export') == before
        run_cli(database, 'submit', 'TEST-A', '--at', hours(-2), '--actor', 'agent:worker')
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 3, 'at': hours(-1)})
        events = worker.call('get_work', {'key': 'TEST-A'})['data']['execution']['events']
        assert events == {'started_at': hours(-3), 'submitted_at': hours(-2), 'verified_at': hours(-1)}, events
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        recorded = [entry['operation'] for entry in history['entries'][-3:]]
        occurred = [next(iter(op['command'].values()))['occurred_at'] for op in recorded]
        assert occurred == [hours(-3), hours(-2), hours(-1)], recorded
        assert all(op['timestamp'] > hours(-1) for op in recorded), recorded
        claim = history['entries'][0]['operation']['command']
        assert list(claim) == ['Claim'] and 'occurred_at' not in claim['Claim'], claim
    finally:
        worker.close()
        reviewer.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-occurrence-') as directory:
        occurrence_smoke(Path(directory))
    print('PASS: CLI/MCP occurrence times for start, submit and verify, their refusals and history')
