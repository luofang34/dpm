#!/usr/bin/env python3
"""Verify conditional work, decision options and branch joins through real CLI and MCP processes."""
import argparse
import copy
import json
import tempfile
import uuid
from pathlib import Path

from smoke_agent import Agent, ROOT, run_cli

FIXTURE = ROOT / 'tests/support/conditional-plan.json'
TRANSCRIPT = []


def log(channel, request, response):
    TRANSCRIPT.append({'channel': channel, 'request': request, 'response': response})
    return response


def cli(database, *args, error=None):
    return log('cli', ['dpm', '--json', *args], run_cli(database, *args, error=error))


def call(agent, name, arguments, error=None):
    return log(f'mcp:{name}', arguments, agent.call(name, arguments, error=error))


def parity(database, worker, label):
    """Every conditional projection is identical through both adapters."""
    pairs = [
        ('project_status', {}, ('status',)),
        ('next_work', {}, ('next',)),
        ('next_work', {'project_keys': ['SUP']}, ('next', '--project-key', 'SUP')),
        ('explain_work', {'key': 'SUP-MERGE'}, ('explain', 'SUP-MERGE')),
        ('explain_work', {'key': 'SUP-A-QUAL'}, ('explain', 'SUP-A-QUAL')),
        ('explain_work', {'key': 'SUP-A-AUDIT'}, ('explain', 'SUP-A-AUDIT')),
        ('get_work', {'key': 'SUP-PKG-A'}, ('show', 'SUP-PKG-A')),
    ]
    for tool, arguments, command in pairs:
        assert call(worker, tool, arguments)['data'] == cli(database, *command), (label, tool)


def applicability_gate(error):
    return next(g['applicability']['state'] for g in error['details']['unmet'] if g['type'] == 'applicability')


def refused_alike(database, agent, tool, arguments, command, code='invalid_command'):
    before = cli(database, 'export')
    remote = call(agent, tool, arguments, error=code)
    local = cli(database, *command, error=code)['error']
    assert remote['message'] == local['message'], (remote, local)
    assert cli(database, 'export') == before, 'refusals change no state'
    return local


def complete(database, key, revision):
    for command in ('claim', 'start', 'submit'):
        cli(database, command, key, '--actor', 'agent:parity')
    cli(database, 'verify', key, '--actor', 'human:reviewer')
    return revision + 4


def smoke(directory):
    database = Path(directory) / 'conditional.sqlite'
    cli(database, 'import', str(FIXTURE))
    worker = Agent(database, 'agent:parity')
    reviewer = Agent(database, 'human:reviewer')
    try:
        parity(database, worker, 'open choice')
        status = cli(database, 'status')
        assert status['p50_finish_hours'] is None and status['open_choices']['scenario_count'] == 2
        assert {s['choices']['DEC-SUPPLIER'] for s in status['open_choices']['scenarios']} == {'A', 'B'}
        merge = cli(database, 'explain', 'SUP-MERGE')
        assert merge['applicability']['state'] == 'awaiting_choice'
        assert 'DEC-SUPPLIER' in {d['key'] for d in cli(database, 'explain', 'SUP-A-QUAL')['context']['decisions']}

        revision = complete(database, 'SUP-DESIGN', 0)
        unknown = refused_alike(database, worker, 'claim_work', {'key': 'SUP-A-QUOTE', 'base_revision': revision},
                                ('claim', 'SUP-A-QUOTE', '--actor', 'agent:parity'))
        assert applicability_gate(unknown) == 'undecided'
        refused_alike(database, reviewer, 'decide_gate',
                      {'decision': 'DEC-SUPPLIER', 'outcome': 'Supplier B', 'base_revision': revision},
                      ('decide', 'DEC-SUPPLIER', 'Supplier B', '--actor', 'human:reviewer'))
        call(reviewer, 'decide_gate', {'decision': 'DEC-SUPPLIER', 'outcome': 'B', 'base_revision': revision})
        revision += 1
        parity(database, worker, 'supplier B selected')
        next_work = cli(database, 'next')
        assert [c['work']['key'] for c in next_work['candidates']] == ['SUP-B-QUOTE']
        skipped = refused_alike(database, worker, 'claim_work', {'key': 'SUP-A-QUAL', 'base_revision': revision},
                                ('claim', 'SUP-A-QUAL', '--actor', 'agent:parity'))
        assert applicability_gate(skipped) == 'not_selected'
        stranded = refused_alike(database, worker, 'claim_work', {'key': 'SUP-A-AUDIT', 'base_revision': revision},
                                 ('claim', 'SUP-A-AUDIT', '--actor', 'agent:parity'))
        assert any(g.get('release', {}).get('state') == 'not_selected' for g in stranded['details']['unmet'])

        revision = complete(database, 'SUP-B-QUOTE', revision)
        cli(database, 'claim', 'SUP-B-QUAL', '--actor', 'agent:parity')
        cli(database, 'start', 'SUP-B-QUAL', '--actor', 'agent:parity')
        revision += 2
        change_after_start(database, worker, reviewer, revision)
    finally:
        worker.close()
        reviewer.close()


def change_after_start(database, worker, reviewer, revision):
    """Switching the choice under started work needs review, is reported, and cancels nothing."""
    refused_alike(database, reviewer, 'decide_gate',
                  {'decision': 'DEC-SUPPLIER', 'outcome': 'A', 'base_revision': revision},
                  ('decide', 'DEC-SUPPLIER', 'A', '--actor', 'human:reviewer'))
    plan = cli(database, 'export')
    proposal = copy.deepcopy(plan)
    old = next(d for d in proposal['decisions'].values() if d['key'] == 'DEC-SUPPLIER')
    old['status'] = 'Superseded'
    new_id = str(uuid.UUID(int=0x0e << 96 | 2))
    replacement = {**old, 'id': new_id, 'key': 'DEC-SUPPLIER-A', 'status': 'Decided', 'outcome': 'A',
                   'rationale': 'Supplier B failed qualification', 'supersedes': old['id']}
    replacement.pop('resolved_at', None)
    proposal['decisions'][new_id] = replacement
    candidate = Path(database).with_name('switch.json')
    candidate.write_text(json.dumps(proposal))
    preview = call(worker, 'propose_change', {'plan': proposal})['data']
    assert preview == cli(database, 'plan', 'diff', str(candidate))
    flagged = {c['key']: c for c in preview['applicability_changes']}
    assert flagged['SUP-B-QUAL']['in_flight'] and flagged['SUP-B-QUAL']['after']['state'] == 'not_selected'
    refused_alike(database, worker, 'apply_change', {'plan': proposal, 'reason': 'switch', 'base_revision': revision},
                  ('plan', 'apply', str(candidate), '--reason', 'switch', '--actor', 'agent:parity'))
    call(reviewer, 'apply_change', {'plan': proposal, 'reason': 'Supplier B failed qualification', 'base_revision': revision})
    kept = cli(database, 'show', 'SUP-B-QUAL')
    assert kept['status'] == 'InProgress' and kept['owner'] == {'kind': 'Agent', 'name': 'parity'}
    assert kept['progress']['scope'] == 'not_selected'
    refused_alike(database, worker, 'submit_work', {'key': 'SUP-B-QUAL', 'base_revision': revision + 1},
                  ('submit', 'SUP-B-QUAL', '--actor', 'agent:parity'))
    parity(database, worker, 'choice switched under started work')
    assert [c['work']['key'] for c in cli(database, 'next')['candidates']] == ['SUP-A-QUOTE']


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--transcript', type=Path)
    options = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='dpm-conditional-') as directory:
        smoke(directory)
    if options.transcript:
        options.transcript.write_text(''.join(json.dumps(entry) + '\n' for entry in TRANSCRIPT))
    print('PASS: CLI/MCP conditional parity: options, undecided/not-selected/stranded gates, branch join and scenarios')
    print('PASS: choice change under started work needs review, is reported and keeps the work; refusals change nothing')
