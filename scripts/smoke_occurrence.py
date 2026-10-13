#!/usr/bin/env python3
"""Claim, start, submit, verify and decide record when their event occurred beside the operation's
commit time, and refuse an occurrence time after the commit or before the task's previous event,
its claim included, alike through the CLI and MCP. A project entered long after it happened then
looks back to when its events occurred."""
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

from smoke_agent import ROOT, Agent, run_cli, run_cli_envelope

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


def historical_smoke(directory):
    """A project recorded decades later: decide, claim, start, submit and verify in event order."""
    database = directory / 'historical.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    worker = Agent(database, 'agent:worker')
    lead = Agent(database, 'human:lead')
    try:
        # The gate was decided in 1945; a claim before then is refused alike by both adapters.
        early = {'key': 'TEST-B', 'base_revision': 0, 'at': '1945-06-01T00:00:00Z'}
        remote = worker.call('claim_work', early, error='invalid_command')
        local = run_cli(database, 'claim', 'TEST-B', '--at', early['at'], '--actor', 'agent:worker', error='invalid_command')['error']
        assert remote == local, (remote, local)
        decided = lead.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Proceed', 'base_revision': 0, 'at': '1945-07-01T00:00:00Z'})['data']
        assert decided['command']['Decide']['occurred_at'] == '1945-07-01T00:00:00Z', decided
        future = (datetime.now(timezone.utc) + timedelta(days=1)).strftime('%Y-%m-%dT%H:%M:%SZ')
        run_cli(database, 'decide', 'TEST-GATE', 'Proceed', '--at', future, '--actor', 'human:lead', error='invalid_command')
        run_cli(database, 'claim', 'TEST-A', '--at', '1945-07-02T00:00:00Z', '--actor', 'agent:worker')
        worker.call('start_work', {'key': 'TEST-A', 'base_revision': 2, 'at': '1945-07-03T00:00:00Z'})
        worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 3, 'at': '1945-08-01T00:00:00Z'})
        run_cli(database, 'verify', 'TEST-A', '--at', '1945-08-02T00:00:00Z', '--actor', 'human:lead')
        claimed = worker.call('claim_work', {'key': 'TEST-B', 'base_revision': 5, 'at': '1945-08-03T00:00:00Z'})['data']
        assert claimed['command']['Claim']['occurred_at'] == '1945-08-03T00:00:00Z', claimed
        assert worker.call('get_work', {'key': 'TEST-B'})['data']['execution']['events'] == {'claimed_at': '1945-08-03T00:00:00Z'}
        # Looking back follows the event times, not the minutes in which they were recorded.
        for at, a, b, complete in [
            ('1945-06-15T00:00:00Z', 'Planned', 'Planned', 0),
            ('1945-07-15T00:00:00Z', 'InProgress', 'Planned', 0),
            ('1945-08-01T12:00:00Z', 'Submitted', 'Planned', 0),
            ('1945-08-04T00:00:00Z', 'Verified', 'Claimed', 1),
        ]:
            status = run_cli_envelope(database, '--clock', at, 'status')
            assert worker.call('project_status', {'as_of': at}) == status
            assert status['revision'] == 6 and status['data']['complete'] == complete, (at, status['data']['complete'])
            for key, expected in (('TEST-A', a), ('TEST-B', b)):
                shown = worker.call('get_work', {'key': key, 'as_of': at})
                assert shown == run_cli_envelope(database, '--clock', at, 'show', key)
                assert shown['data']['execution']['status'] == expected, (at, key, shown['data']['execution'])
        gate = run_cli(database, '--clock', '1945-06-15T00:00:00Z', 'export')['decisions']
        assert all(d['status'] == 'Open' for d in gate.values() if d['key'] == 'TEST-GATE'), gate
    finally:
        worker.close()
        lead.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-occurrence-') as directory:
        occurrence_smoke(Path(directory))
        historical_smoke(Path(directory))
    print('PASS: CLI/MCP occurrence times for claim, start, submit, verify and decide, their refusals, history and look back')
