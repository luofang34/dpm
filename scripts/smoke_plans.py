#!/usr/bin/env python3
"""Check reviewed planning and durable history through the real CLI and agent transport."""
import copy
import json
import sqlite3
import subprocess
import tempfile
import uuid
from pathlib import Path

from smoke_agent import Agent, CLI, ROOT, run_cli


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
                work['execution']['status'] = 'Proposed'
        candidate.write_text(json.dumps(proposal))
        preview = worker.call('propose_change', {'plan': proposal})['data']
        assert preview == run_cli(database, 'plan', 'diff', str(candidate))
        assert preview['base_revision'] == 0 and preview['changes']
        assert run_cli(database, 'export') == initial
        refused = worker.call('apply_change', {'plan': proposal, 'reason': 'self-approve', 'base_revision': 0}, error='invalid_command')
        assert refused['message'] == run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'self-approve', '--actor', 'agent:planner', error='invalid_command')['error']['message']
        approved = run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Prepare contracts for review', '--actor', 'human:reviewer')
        assert approved['resulting_revision'] == 1
        assert worker.call('next_work', {})['data']['candidates'] == []
        reviewer.call('ratify_contract', {'key': 'TEST-A', 'base_revision': 1})
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 2})
        changed = worker.call('export_plan', {})['data']
        next(w for w in changed['work_items'].values() if w['key'] == 'TEST-F')['title'] = 'Future integration contract'
        candidate.write_text(json.dumps(changed))
        diff = worker.call('propose_change', {'plan': changed})['data']
        assert diff == run_cli(database, 'plan', 'diff', str(candidate))
        assert diff['changes'][0]['fields'] == ['title']
        clarified = reviewer.call('apply_change', {'plan': changed, 'reason': 'Clarify future integration', 'base_revision': 3})['data']
        # The log records the reviewed difference, not the proposal: exactly what plan diff showed.
        assert clarified['command'] == {'ApplyChange': {'changes': diff['changes'], 'reason': 'Clarify future integration'}}
        assert len(json.dumps(clarified['command'])) * 2 < len(json.dumps(changed)), clarified
        assert 'artifacts' not in {c['collection'] for c in approved['command']['ApplyChange']['changes']}
        current = run_cli(database, 'export')
        assert current['revision'] == 4
        stale = worker.call('propose_change', {'plan': changed}, error='revision_conflict')
        assert stale['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='revision_conflict')['error']['message']
        reject_invalid(database, worker, reviewer, candidate, current)
        reject_nonportable(database, worker, reviewer, candidate, current)
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        assert len(history['entries']) == 4 and history['revision'] == 4
        assert history['entries'][0]['operation'] == approved
        assert history['entries'][-1]['operation'] == clarified
        first = history['entries'][0]['operation']['command']['ApplyChange']['changes']
        assert {c['collection'] for c in first} >= {'work_items', 'dependencies', 'decisions'}, first
        assert all(c['before'] is None for c in first if c['collection'] == 'work_items'), first
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
    for error_case in ['acceptance', 'cycle', 'duplicate_edge', 'lifecycle', 'gate', 'protected', 'noop']:
        bad = copy.deepcopy(current)
        task = next(w for w in bad['work_items'].values() if w['key'] == 'TEST-A')
        if error_case == 'acceptance':
            task['contract']['acceptance'] = [{'text': 'Lower the accepted result'}]
        elif error_case == 'cycle':
            edge = bad['dependencies'][0]
            reverse = {**edge, 'predecessor': edge['successor'], 'successor': edge['predecessor']}
            reverse['id'] = str(uuid.uuid4())
            bad['dependencies'].append(reverse)
        elif error_case == 'duplicate_edge':
            bad['dependencies'].append({**bad['dependencies'][-1], 'kind': 'StartStart'})
        elif error_case == 'lifecycle':
            task['execution']['status'] = 'Verified'
        elif error_case == 'protected':
            task['title'] = 'Rewritten while claimed'
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


def reject_nonportable(database, worker, reviewer, candidate, current):
    for case in ('missing_id', 'unknown_asset_field', 'credential_remote'):
        bad = copy.deepcopy(current)
        asset = next(iter(bad['assets'].values()))
        if case == 'missing_id':
            del bad['dependencies'][0]['id']
        elif case == 'unknown_asset_field':
            asset['kind']['GitRepository']['unrecognized_policy'] = True
        else:
            asset['kind']['GitRepository']['remotes'] = ['https://EXAMPLE_TOKEN@example.invalid/repo']
        code = 'invalid_command' if case == 'credential_remote' else 'invalid_request'
        candidate.write_text(json.dumps(bad))
        worker.call('propose_change', {'plan': bad}, error=code)
        run_cli(database, 'plan', 'diff', str(candidate), error=code)
        reviewer.call('apply_change', {'plan': bad, 'reason': 'Invalid portable input',
                                      'base_revision': 4}, error=code)
        run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Invalid portable input', error=code)
        assert run_cli(database, 'export') == current


def key_map(plan):
    return {w['key']: w['id'] for w in plan['work_items'].values()}


def replacement(directory):
    database = directory / 'replacement.sqlite'
    candidate = directory / 'replacement.json'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:planner')
    reviewer = Agent(database, 'human:reviewer')
    try:
        plan = run_cli(database, 'export')
        ids = key_map(plan)
        project = next(iter(plan['projects']))
        choice = {'id': str(uuid.uuid4()), 'key': 'TEST-CHOICE', 'project': project,
                  'question': 'Which input format?', 'status': 'Open', 'outcome': None,
                  'rationale': 'Existing tooling reads JSON',
                  'related_work': [ids['TEST-A'], ids['TEST-C']], 'blocks': []}
        plan['decisions'][choice['id']] = choice
        reviewer.call('apply_change', {'plan': plan, 'reason': 'Record the format question', 'base_revision': 0})
        reviewer.call('decide_gate', {'decision': 'TEST-CHOICE', 'outcome': 'JSON', 'base_revision': 1})
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 2})
        current = run_cli(database, 'export')
        replace_invalid(database, worker, reviewer, candidate, current, choice['id'])
        proposal = superseding(current, choice['id'], [ids['TEST-A'], ids['TEST-F']])
        candidate.write_text(json.dumps(proposal))
        preview = worker.call('propose_change', {'plan': proposal})['data']
        assert preview == run_cli(database, 'plan', 'diff', str(candidate))
        assert [(a['key'], a['status']) for a in preview['affected_work']] == [
            ('TEST-A', 'Claimed'), ('TEST-C', 'Planned'), ('TEST-F', 'Planned')]
        old_change = next(c for c in preview['changes'] if c['id'] == choice['id'])
        assert old_change['fields'] == ['status']
        refused = worker.call('apply_change', {'plan': proposal, 'reason': 'self-approve', 'base_revision': 3}, error='invalid_command')
        assert refused['message'] == run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'self-approve', '--actor', 'agent:planner', error='invalid_command')['error']['message']
        readiness = worker.call('next_work', {'probabilistic': False})['data']
        applied = run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Reviewers edit TOML', '--actor', 'human:reviewer')
        assert applied['resulting_revision'] == 4
        assert worker.call('next_work', {'probabilistic': False})['data'] == readiness
        explained = worker.call('explain_work', {'key': 'TEST-A'})['data']
        assert explained == run_cli(database, 'explain', 'TEST-A')
        decisions = {d['key']: d for d in explained['context']['decisions']}
        assert decisions['TEST-CHOICE']['status'] == 'Superseded'
        assert decisions['TEST-CHOICE']['rationale'] == 'Existing tooling reads JSON'
        assert decisions['TEST-CHOICE-2']['supersedes'] == choice['id']
        # TEST-C is linked only to the superseded choice and must still see its replacement.
        only_old = worker.call('explain_work', {'key': 'TEST-C'})['data']
        assert only_old == run_cli(database, 'explain', 'TEST-C')
        assert {d['key'] for d in only_old['context']['decisions']} == {'TEST-CHOICE', 'TEST-CHOICE-2'}
        stale = worker.call('propose_change', {'plan': proposal}, error='revision_conflict')
        assert stale['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='revision_conflict')['error']['message']
    finally:
        worker.close()
        reviewer.close()
    reopened = Agent(database, 'agent:reader')
    try:
        assert reopened.call('explain_work', {'key': 'TEST-A'})['data'] == explained
    finally:
        reopened.close()


def superseding(plan, old, related):
    proposal = copy.deepcopy(plan)
    proposal['decisions'][old]['status'] = 'Superseded'
    new = {**proposal['decisions'][old], 'id': str(uuid.uuid4()), 'key': 'TEST-CHOICE-2',
           'status': 'Decided', 'outcome': 'TOML', 'rationale': 'Reviewers edit TOML by hand',
           'related_work': related, 'blocks': [], 'supersedes': old}
    # A proposal carries no resolution time; apply_change records the replacement's own.
    new.pop('resolved_at', None)
    proposal['decisions'][new['id']] = new
    return proposal


def replace_invalid(database, worker, reviewer, candidate, current, choice):
    gate = next(d['id'] for d in current['decisions'].values() if d['key'] == 'TEST-GATE')
    for error_case in ['open_gate', 'dangling', 'rewrite', 'unlinked']:
        if error_case == 'open_gate':
            bad = superseding(current, gate, [])
            bad['decisions'][gate]['outcome'] = None
        else:
            bad = superseding(current, choice, [])
        new = next(d for d in bad['decisions'].values() if d['key'] == 'TEST-CHOICE-2')
        if error_case == 'dangling':
            new['supersedes'] = str(uuid.uuid4())
        elif error_case == 'rewrite':
            bad['decisions'][choice]['rationale'] = 'Rewritten reasoning'
        elif error_case == 'unlinked':
            del bad['decisions'][new['id']]
        candidate.write_text(json.dumps(bad))
        remote = worker.call('propose_change', {'plan': bad}, error='invalid_command')
        assert remote['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='invalid_command')['error']['message']
        if error_case == 'open_gate':
            assert 'use decide for an open outcome' in remote['message'], remote
        remote = reviewer.call('apply_change', {'plan': bad, 'reason': 'Invalid replacement', 'base_revision': 3}, error='invalid_command')
        assert remote['message'] == run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Invalid replacement', error='invalid_command')['error']['message']
        assert run_cli(database, 'export') == current


def authoring(directory):
    """An agent can go from an empty workspace to executable work using only the published contract."""
    unconfigured = directory / 'no-project'
    unconfigured.mkdir()
    shown = subprocess.run([str(CLI), '--json', 'plan', 'schema'], cwd=unconfigured, capture_output=True, text=True, timeout=15)
    assert shown.returncode == 0, shown
    shown = json.loads(shown.stdout)
    # No workspace was read, so the envelope names no revision.
    assert shown['revision'] is None, shown
    schema = shown['data']
    database = directory / 'authoring.sqlite'
    template_file = directory / 'template.json'
    run_cli(database, 'init', 'Authored workspace')
    worker = Agent(database, 'agent:author')
    try:
        assert worker.call('plan_schema', {})['data'] == schema
        template = run_cli(database, 'plan', 'template')
        assert worker.call('plan_template', {})['data'] == template
        assert template['workspace'] == run_cli(database, 'export')['workspace'] and template['revision'] == 0
        try:
            import jsonschema
        except ImportError:
            jsonschema = None
        if jsonschema is not None:
            jsonschema.Draft202012Validator(schema).validate(template)
        template_file.write_text(json.dumps(template))
        assert worker.call('propose_change', {'plan': template})['data'] == run_cli(database, 'plan', 'diff', str(template_file))
        run_cli(database, 'plan', 'apply', str(template_file), '--reason', 'Start from the template', '--actor', 'human:lead')
        run_cli(database, 'ratify', 'TEMPLATE-DESIGN', '--actor', 'human:lead')
        worker.call('claim_work', {'key': 'TEMPLATE-DESIGN', 'base_revision': 2})
        refused = worker.call('plan_template', {}, error='invalid_request')
        assert refused == run_cli(database, 'plan', 'template', error='invalid_request')['error']
    finally:
        worker.close()
    assert run_cli(directory / 'imported.sqlite', 'import', str(template_file))['revision'] == 0


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-plan-') as temporary:
        smoke(Path(temporary))
        replacement(Path(temporary))
        authoring(Path(temporary))
    print('PASS: empty workspace to reviewed graph, CLI/MCP plan diff/apply/history parity, protected execution, stale proposals, decision replacement with affected work, durable restart and schema/template authoring')
