#!/usr/bin/env python3
"""Start, submit and verify record when their event occurred beside the operation's commit time,
and refuse an occurrence time after the commit or before the task's previous event, its claim
included, alike through the CLI and MCP."""
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from smoke_agent import ROOT, Agent, run_cli

FIXTURE = ROOT / 'tests/support/execution-plan.json'


def parse(text):
    return datetime.fromisoformat(text.replace('Z', '+00:00'))


def stamp(moment):
    """RFC 3339 as the model serializes a millisecond time: fractional digits only when nonzero."""
    text = moment.strftime('%Y-%m-%dT%H:%M:%S')
    milliseconds = moment.microsecond // 1000
    return f'{text}.{milliseconds:03d}Z' if milliseconds else f'{text}Z'


def occurrence_smoke(directory):
    database = directory / 'occurrence.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    now = datetime.now(timezone.utc).replace(microsecond=0)

    def hours(offset):
        return (now + timedelta(hours=offset)).strftime('%Y-%m-%dT%H:%M:%SZ')

    run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:worker')
    claimed_at = run_cli(database, 'history')['entries'][0]['operation']['timestamp']
    # Occurrence times just after the claim, and so before the later commits that record them.
    claim = parse(claimed_at)
    base = claim.replace(microsecond=claim.microsecond // 1000 * 1000) + timedelta(milliseconds=1)

    def after(milliseconds):
        return stamp(base + timedelta(milliseconds=milliseconds))

    worker = Agent(database, 'agent:worker')
    reviewer = Agent(database, 'human:reviewer')
    try:
        assert worker.call('get_work', {'key': 'TEST-A'})['data']['execution']['events'] == {'claimed_at': claimed_at}
        before = run_cli(database, 'export')
        # A claim reserves the work at its commit time; nothing the task records may precede it.
        remote = worker.call('start_work', {'key': 'TEST-A', 'base_revision': 1, 'at': hours(-3)}, error='invalid_command')
        local = run_cli(database, 'start', 'TEST-A', '--at', hours(-3), '--actor', 'agent:worker', error='invalid_command')['error']
        assert remote == local and 'before its previous recorded event' in local['message'] and 'TEST-A' in local['message'], (remote, local)
        assert run_cli(database, 'export') == before
        started = worker.call('start_work', {'key': 'TEST-A', 'base_revision': 1, 'at': after(0)})['data']
        assert started['command']['Start']['occurred_at'] == after(0) and started['timestamp'] > after(0), started
        assert worker.call('get_work', {'key': 'TEST-A'})['data']['execution']['events'] == {'started_at': after(0)}
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
        run_cli(database, 'submit', 'TEST-A', '--at', after(1), '--actor', 'agent:worker')
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 3, 'at': after(2)})
        events = worker.call('get_work', {'key': 'TEST-A'})['data']['execution']['events']
        assert events == {'started_at': after(0), 'submitted_at': after(1), 'verified_at': after(2)}, events
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        recorded = [entry['operation'] for entry in history['entries'][-3:]]
        occurred = [next(iter(op['command'].values()))['occurred_at'] for op in recorded]
        assert occurred == [after(0), after(1), after(2)], recorded
        assert all(parse(op['timestamp']) > parse(after(2)) for op in recorded), recorded
        claim = history['entries'][0]['operation']['command']
        assert list(claim) == ['Claim'] and 'occurred_at' not in claim['Claim'], claim
    finally:
        worker.close()
        reviewer.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-occurrence-') as directory:
        occurrence_smoke(Path(directory))
    print('PASS: CLI/MCP occurrence times for start, submit and verify, their refusals and history')
