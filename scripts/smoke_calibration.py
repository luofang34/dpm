#!/usr/bin/env python3
"""Calibration from history, the opt-in calibrated forecast and the holding-claims advisory of
next answer alike through the CLI and MCP; the repository preview reports an empty history."""
import json
import subprocess
import tempfile
from datetime import timedelta
from pathlib import Path

from smoke_agent import CLI, ROOT, Agent, pinned_clock, run_cli, run_cli_envelope
from smoke_occurrence import parse, stamp

FIXTURE = ROOT / 'tests/support/execution-plan.json'


def calibration_smoke(directory):
    database = directory / 'calibration.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    run_cli(database, 'claim', 'TEST-A', '--actor', 'agent:worker')
    claimed = parse(run_cli(database, 'history')['entries'][0]['operation']['timestamp'])
    base = claimed.replace(microsecond=claimed.microsecond // 1000 * 1000)

    def after(milliseconds):
        return stamp(base + timedelta(milliseconds=milliseconds))

    # Backfilled with occurrence times, so the pair is measured even though it was committed at once.
    run_cli(database, 'start', 'TEST-A', '--at', after(1), '--actor', 'agent:worker')
    run_cli(database, 'submit', 'TEST-A', '--at', after(2), '--actor', 'agent:worker')
    run_cli(database, 'verify', 'TEST-A', '--actor', 'human:reviewer')
    run_cli(database, 'decide', 'TEST-GATE', 'Proceed', '--actor', 'human:reviewer')
    # Recorded in one sitting without occurrence times: excluded from calibration and counted.
    for command in ('claim', 'start', 'submit'):
        run_cli(database, command, 'TEST-B', '--actor', 'agent:worker')
    run_cli(database, 'verify', 'TEST-B', '--actor', 'human:reviewer')
    run_cli(database, 'claim', 'TEST-D', '--actor', 'agent:worker')

    at = pinned_clock()
    agent = Agent(database, 'agent:worker', clock=at)
    try:
        calibration = agent.call('get_calibration', {})
        calibrated = agent.call('project_status', {'calibrated': True})
        plain = agent.call('project_status', {})
        advised = agent.call('next_work', {'actor': 'agent:worker', 'probabilistic': False})
        anonymous = agent.call('next_work', {'probabilistic': False})
        agent.call('next_work', {'actor': 'worker'}, error='invalid_request')
    finally:
        agent.close()
    assert calibration == run_cli_envelope(database, '--clock', at, 'calibration')
    assert calibrated == run_cli_envelope(database, '--clock', at, 'status', '--calibrated')
    assert plain == run_cli_envelope(database, '--clock', at, 'status')
    assert advised == run_cli_envelope(database, '--clock', at, 'next', '--actor', 'agent:worker', '--deterministic-only')
    assert anonymous == run_cli_envelope(database, '--clock', at, 'next', '--deterministic-only')

    report = calibration['data']
    assert [s['key'] for s in report['estimates']['samples']] == ['TEST-A'], report['estimates']
    assert report['estimates']['excluded'] == [{'reason': 'bulk_recorded', 'count': 1, 'keys': ['TEST-B']}]
    assert report['history']['operations'] == 10 and report['rules']['min_samples'] == 5
    assert report['flow']['reliability']['total']['verified'] == 2
    assert [w['key'] for w in report['flow']['aging']] == ['TEST-D']
    # Too few samples: nothing is applied, and the forecast is the plain one plus its explanation.
    applied = calibrated['data'].pop('calibration')
    assert calibrated == plain and 'calibration' not in plain['data']
    assert all(not f['applied'] and f['factor'] == 1.0 for f in applied['factors']), applied
    assert not applied['review_delay']['applied'] and applied['review_delay']['hours'] == 0.0
    # The advisory explains held work and never changes the candidates.
    assert 'advisories' not in anonymous['data']
    assert advised['data']['candidates'] == anonymous['data']['candidates']
    assert advised['data']['advisories'][0]['kind'] == 'holding_claims'
    assert advised['data']['advisories'][0]['holding'] == ['TEST-D']

    text = subprocess.run([str(CLI), '--database', str(database), 'calibration'], cwd=ROOT,
                          capture_output=True, text=True, timeout=15, check=True).stdout
    assert 'Estimates: 1 sample(s)' in text and 'excluded BulkRecorded: 1' in text, text

    # The repository's read-only preview has no history, so the report has no samples.
    preview = subprocess.run([str(CLI), '--json', 'calibration'], cwd=ROOT, capture_output=True,
                             text=True, timeout=15, check=True)
    value = json.loads(preview.stdout)
    assert value['lineage_id'] is None and value['data']['history']['operations'] == 0, value
    assert value['data']['estimates']['samples'] == [], value


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-calibration-') as directory:
        calibration_smoke(Path(directory))
    print('PASS: CLI/MCP calibration, calibrated status and next advisories')
