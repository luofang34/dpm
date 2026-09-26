#!/usr/bin/env python3
"""Verify real CLI/MCP processes share structured queries and mutation behavior."""
import json
import subprocess
import tempfile
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / 'target/release/dpm'
MCP = ROOT / 'target/release/dpm-mcp'


class Agent:
    def __init__(self, database, actor, *, cwd=ROOT, project=None, env=None):
        selection = ['--db', str(database)] if database is not None else []
        if project is not None:
            selection = ['--project', str(project)]
        self.process = subprocess.Popen(
            [str(MCP), *selection, '--actor', actor],
            cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
        )
        self.sequence = 0
        result = self.request('initialize', {
            'protocolVersion': '2025-11-25', 'capabilities': {},
            'clientInfo': {'name': 'dpm-parity', 'version': '1'},
        })
        assert result['protocolVersion'] == '2025-11-25'
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n')
        self.process.stdin.flush()

    def request(self, method, params):
        self.sequence += 1
        self.process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': self.sequence, 'method': method, 'params': params}) + '\n')
        self.process.stdin.flush()
        response = json.loads(self.process.stdout.readline())
        assert response['id'] == self.sequence and 'error' not in response, response
        return response['result']

    def call(self, name, arguments, error=None):
        result = self.request('tools/call', {'name': name, 'arguments': arguments})
        value = result['structuredContent']
        if error:
            assert result['isError'] and value['code'] == error, result
        else:
            assert not result['isError'], result
        return value

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=5)
        assert self.process.returncode == 0, self.process.stderr.read()
        self.process.stdout.close()
        self.process.stderr.close()


def run_cli(database, *args, error=None):
    result = subprocess.run([str(CLI), '--database', str(database), '--json', *args], cwd=ROOT, capture_output=True, text=True, timeout=15)
    value = json.loads(result.stdout)
    if error:
        assert result.returncode != 0 and value['error']['code'] == error, (args, value)
    else:
        assert result.returncode == 0, (args, value, result.stderr)
    return value


def smoke(database):
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    reviewer = Agent(database, 'human:reviewer')
    try:
        names = {tool['name'] for tool in worker.request('tools/list', {})['tools']}
        assert {'project_status', 'next_work', 'get_work', 'explain_work', 'claim_work', 'report_blocker', 'unblock_work', 'submit_work', 'verify_work', 'report_progress', 'add_artifact', 'attach_git_head', 'decide_gate', 'ratify_contract', 'reject_work', 'workspace_list', 'workspace_register', 'export_plan', 'propose_change', 'apply_change', 'history', 'link_external', 'unlink_external'} == names
        pairs = [
            ('project_status', {}, ('status',)),
            ('next_work', {}, ('next',)),
            ('next_work', {'capabilities': ['unrelated']}, ('next', '--capability', 'unrelated')),
            ('get_work', {'key': 'TEST-A'}, ('show', 'TEST-A')),
            ('explain_work', {'key': 'TEST-B'}, ('explain', 'TEST-B')),
        ]
        for tool, arguments, command in pairs:
            assert worker.call(tool, arguments)['data'] == run_cli(database, *command)
        missing_pairs = [
            ('get_work', {'key': 'missing'}, ('show', 'missing')),
            ('claim_work', {'key': 'missing', 'base_revision': 0}, ('claim', 'missing')),
            ('decide_gate', {'decision': 'missing', 'outcome': 'choice', 'base_revision': 0}, ('decide', 'missing', 'choice')),
        ]
        for tool, arguments, command in missing_pairs:
            agent_error = worker.call(tool, arguments, error='not_found')
            cli_error = run_cli(database, *command, error='not_found')['error']
            assert agent_error['message'] == cli_error['message']
        context = worker.call('explain_work', {'key': 'TEST-B'})['data']['context']
        assert context['requirements'] and context['decisions'][0]['key'] == 'TEST-GATE'
        worker.call('claim_work', {'key': 'TEST-B', 'base_revision': 0}, error='invalid_command')
        run_cli(database, 'claim', 'TEST-B', error='invalid_command')
        agent_error = worker.call('claim_work', {'key': 'TEST-M1', 'base_revision': 0}, error='invalid_command')
        cli_error = run_cli(database, 'claim', 'TEST-M1', error='invalid_command')['error']
        assert agent_error['message'] == cli_error['message'] and 'not a task' in cli_error['message']
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 50, 'base_revision': 0}, error='invalid_command')
        run_cli(database, 'progress', 'TEST-A', '50', error='invalid_command')
        claim = worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})['data']
        assert claim['resulting_revision'] == 1
        run_cli(database, 'progress', 'TEST-A', '25', '--actor', 'agent:parity')
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 50, 'note': 'Half the acceptance work is implemented', 'base_revision': 2})
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 101, 'base_revision': 3}, error='invalid_command')
        run_cli(database, 'progress', 'TEST-A', '101', '--actor', 'agent:parity', error='invalid_command')
        shown = worker.call('get_work', {'key': 'TEST-A'})['data']
        assert shown == run_cli(database, 'show', 'TEST-A')
        assert shown['progress'] == {'percent_complete': 50.0, 'verified': False}
        assert shown['reported_progress_percent'] == 50 and shown['status'] == 'InProgress'
        summary = worker.call('project_status', {})['data']
        assert summary == run_cli(database, 'status') and summary['progress']['percent_complete'] > 0
        run_cli(database, '--base-revision', '0', 'claim', 'TEST-A', error='revision_conflict')
        worker.call('attach_git_head', {'key': 'TEST-A', 'resource': 'TEST-REPO', 'base_revision': 3})
        run_cli(database, 'block', 'TEST-A', 'Waiting for fixture', '--actor', 'agent:parity')
        assert worker.call('next_work', {})['data']['candidates'] == []
        worker.call('unblock_work', {'key': 'TEST-A', 'base_revision': 5})
        worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 6, 'note': 'Acceptance evidence reviewed'})
        worker.call('verify_work', {'key': 'TEST-A', 'base_revision': 7}, error='invalid_command')
        run_cli(database, 'verify', 'TEST-A', '--actor', 'agent:parity', error='invalid_command')
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 7})
        assert worker.call('next_work', {})['data']['candidates'] == []
        reviewer.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Accept the verified input', 'base_revision': 8})
        next_work = worker.call('next_work', {})['data']
        assert next_work == run_cli(database, 'next')
        assert {candidate['work']['key'] for candidate in next_work['candidates']} == {'TEST-B', 'TEST-D'}
        assert worker.call('project_status', {})['data']['revision'] == 9
    finally:
        worker.close()
        reviewer.close()


def review_smoke(directory):
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    task = next(w for w in plan['work_items'].values() if w['key'] == 'TEST-A')
    task['status'] = 'Proposed'
    fixture = directory / 'proposed.json'
    fixture.write_text(json.dumps(plan))
    database = directory / 'review.sqlite'
    run_cli(database, 'import', str(fixture))
    worker = Agent(database, 'agent:worker')
    reviewer = Agent(database, 'human:reviewer')
    try:
        for tool, args, command in [
            ('ratify_contract', {}, ('ratify', 'TEST-A', '--actor', 'agent:worker')),
            ('reject_work', {'reason': 'missing result'}, ('reject', 'TEST-A', 'missing result', '--actor', 'agent:worker')),
        ]:
            remote = worker.call(tool, {'key': 'TEST-A', 'base_revision': 0, **args}, error='invalid_command')
            local = run_cli(database, *command, error='invalid_command')['error']
            assert remote['message'] == local['message']
        reviewer.call('ratify_contract', {'key': 'TEST-A', 'base_revision': 0})
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 1})
        run_cli(database, 'submit', 'TEST-A', '--actor', 'agent:worker')
        review = reviewer.call('reject_work', {'key': 'TEST-A', 'reason': 'Missing acceptance evidence', 'base_revision': 3})
        assert review['data']['command']['Reject']['reason'] == 'Missing acceptance evidence'
        detail = worker.call('explain_work', {'key': 'TEST-A'})['data']
        assert detail == run_cli(database, 'explain', 'TEST-A')
        assert detail['work']['status'] == 'InProgress'
        assert detail['work']['last_rejection']['reason'] == 'Missing acceptance evidence'
        assert worker.call('next_work', {})['data']['candidates'] == []
        run_cli(database, 'submit', 'TEST-A', '--actor', 'agent:worker')
        run_cli(database, 'reject', 'TEST-A', 'Still missing evidence', '--actor', 'human:reviewer')
        assert worker.call('get_work', {'key': 'TEST-A'})['data'] == run_cli(database, 'show', 'TEST-A')
    finally:
        worker.close()
        reviewer.close()
    # Exercise successful CLI ratification independently of the already approved contract.
    second = directory / 'ratify.sqlite'
    run_cli(second, 'import', str(fixture))
    result = run_cli(second, 'ratify', 'TEST-A', '--actor', 'human:reviewer')
    assert 'RatifyContract' in result['command'] and result['resulting_revision'] == 1


def scoped_plan():
    """Ready tasks across a project subtree, two repositories and a resource-less task."""
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    template = next(w for w in plan['work_items'].values() if w['key'] == 'TEST-A')
    edge = plan['dependencies'][0]
    root = next(iter(plan['projects']))
    repo_a = next(iter(plan['resources']))
    sub, other, repo_b = (str(uuid.uuid4()) for _ in range(3))
    plan['projects'][sub] = {**plan['projects'][root], 'id': sub, 'key': 'SUB', 'parent': root}
    plan['projects'][other] = {**plan['projects'][root], 'id': other, 'key': 'OTHER'}
    plan['resources'][repo_b] = {**plan['resources'][repo_a], 'id': repo_b, 'key': 'REPO-B'}
    plan['decisions'], plan['risks'], plan['work_items'], plan['dependencies'] = {}, {}, {}, []
    tasks = [
        ('W-A', root, [(repo_a, 'Write')], 'P2'), ('W-B', other, [(repo_b, 'Write')], 'P0'),
        ('W-AB', sub, [(repo_a, 'Write'), (repo_b, 'Write')], 'P2'), ('R-A', sub, [(repo_a, 'Read')], 'P3'),
        ('NOCODE', other, [], 'P1'), ('DEP', root, [(repo_a, 'Write')], 'P0'),
    ]
    ids = {}
    for key, project, needs, priority in tasks:
        ids[key] = str(uuid.uuid4())
        plan['work_items'][ids[key]] = {
            **template, 'id': ids[key], 'key': key, 'project': project, 'priority': priority,
            'resources': [{'resource': r, 'access': a} for r, a in needs],
        }
    plan['dependencies'].append({**edge, 'predecessor': ids['W-B'], 'successor': ids['DEP']})
    return plan


def scope_smoke(directory):
    fixture = directory / 'scoped.json'
    fixture.write_text(json.dumps(scoped_plan()))
    database = directory / 'scoped.sqlite'
    run_cli(database, 'import', str(fixture))
    before = run_cli(database, 'export')
    agent = Agent(database, 'agent:scope')
    try:
        results = {}
        for name, arguments, flags in [
            ('all', {}, ()),
            ('repo-a', {'resource_keys': ['TEST-REPO']}, ('--resource-key', 'TEST-REPO')),
            ('both', {'resource_keys': ['TEST-REPO', 'REPO-B']}, ('--resource-key', 'TEST-REPO', '--resource-key', 'REPO-B')),
            ('subtree', {'project_keys': ['TEST']}, ('--project-key', 'TEST')),
            ('other', {'project_keys': ['OTHER']}, ('--project-key', 'OTHER')),
            ('intersect', {'project_keys': ['OTHER'], 'resource_keys': ['TEST-REPO']}, ('--project-key', 'OTHER', '--resource-key', 'TEST-REPO')),
            ('limited', {'project_keys': ['TEST'], 'limit': 1}, ('--project-key', 'TEST', '--limit', '1')),
        ]:
            remote = agent.call('next_work', {'probabilistic': False, **arguments})['data']
            assert remote == run_cli(database, 'next', '--deterministic-only', *flags), name
            assert remote['result_version'] == 1
            results[name] = remote
        keys = lambda r: sorted(c['work']['key'] for c in r['candidates'])
        order = [c['work']['key'] for c in results['all']['candidates']]
        assert results['all']['outside_scope'] == {'count': 0, 'higher_ranked_count': 0, 'keys': []}
        assert keys(results['repo-a']) == ['R-A', 'W-A'] and 'NOCODE' in results['repo-a']['outside_scope']['keys']
        assert keys(results['both']) == ['R-A', 'W-A', 'W-AB', 'W-B']
        assert results['both']['outside_scope']['keys'] == ['NOCODE']
        assert keys(results['subtree']) == ['R-A', 'W-A', 'W-AB'] and keys(results['other']) == ['NOCODE', 'W-B']
        assert results['intersect']['candidates'] == []
        assert results['intersect']['outside_scope']['higher_ranked_count'] == results['intersect']['eligible_count']
        assert 'DEP' not in order
        for result in results.values():
            ranks = [c['global_rank'] for c in result['candidates']]
            assert ranks == sorted(ranks) and all(order[r - 1] == c['work']['key'] for r, c in zip(ranks, result['candidates']))
            best = ranks[0] if ranks else None
            higher = result['outside_scope']['keys'][:result['outside_scope']['higher_ranked_count']]
            assert higher == [k for k in order if best is None or order.index(k) < best - 1]
        assert results['repo-a']['outside_scope']['keys'][0] == 'W-B'
        limited = results['limited']
        assert len(limited['candidates']) == 1 and limited['in_scope_count'] == 3
        assert limited['outside_scope'] == results['subtree']['outside_scope']
        for arguments, flags in [({'project_keys': ['MISSING']}, ('--project-key', 'MISSING')),
                                 ({'resource_keys': ['MISSING']}, ('--resource-key', 'MISSING'))]:
            remote = agent.call('next_work', arguments, error='not_found')
            assert remote['message'] == run_cli(database, 'next', *flags, error='not_found')['error']['message']
        agent.call('next_work', {'project': 'TEST'}, error='invalid_request')
    finally:
        agent.close()
    assert run_cli(database, 'export') == before
    assert run_cli(database, 'history')['entries'] == []


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-agent-') as directory:
        smoke(Path(directory) / 'plan.sqlite')
        review_smoke(Path(directory))
        scope_smoke(Path(directory))
    print('PASS: CLI/MCP query parity, revision conflicts, evidence, blockers, gates and independent verification')
    print('PASS: scoped next parity, outside-scope visibility, limits and unknown scope keys without state change')
