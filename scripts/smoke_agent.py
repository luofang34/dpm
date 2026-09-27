#!/usr/bin/env python3
"""Verify real CLI/MCP processes share structured queries and mutation behavior."""
import json
import os
import subprocess
import tempfile
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def release_binary(name, override):
    """Locate a release binary the way Cargo placed it, so reviewers need no symlinks.

    An explicit override variable wins; otherwise CARGO_TARGET_DIR (relative to the current
    directory, as Cargo resolves it) or the workspace target directory.
    """
    explicit = os.environ.get(override)
    if explicit:
        path = Path(explicit)
    else:
        path = Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target') / 'release' / name
    path = path.resolve()
    if not path.is_file():
        raise SystemExit(f'{path}: missing {name}; run cargo build --workspace --release or set {override} / CARGO_TARGET_DIR')
    return path


CLI = release_binary('dpm', 'DPM_BIN')
MCP = release_binary('dpm-mcp', 'DPM_MCP_BIN')


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


# The CLI requires an explicit actor for every mutation; these smoke defaults name who acts.
WORKER_COMMANDS = {'claim', 'start', 'progress', 'block', 'unblock', 'submit', 'artifact', 'attach-git-head',
                   'link-external', 'unlink-external'}
REVIEW_COMMANDS = {'verify', 'reject', 'ratify', 'decide', 'waive-dependency', 'restore-dependency', 'revalidate-basis'}


def with_actor(args):
    rest = [str(a) for a in args]
    while rest and rest[0] in {'--base-revision', '--project', '--database', '--db'}:
        rest = rest[2:]
    command = rest[:2] if rest[:1] == ['plan'] else rest[:1]
    if '--actor' in rest or not command:
        return list(args)
    if command == ['plan', 'apply'] or command[0] in REVIEW_COMMANDS:
        return [*args, '--actor', 'human:local']
    if command[0] in WORKER_COMMANDS:
        return [*args, '--actor', 'agent:local']
    return list(args)


def run_cli(database, *args, error=None):
    result = subprocess.run([str(CLI), '--database', str(database), '--json', *with_actor(args)], cwd=ROOT, capture_output=True, text=True, timeout=15)
    value = json.loads(result.stdout)
    if error:
        assert result.returncode != 0 and value['error']['code'] == error, (args, value)
    else:
        assert result.returncode == 0, (args, value, result.stderr)
    return value


def smoke(database):
    usage = subprocess.run([str(MCP), '--help'], capture_output=True, text=True, timeout=15, check=True).stdout
    assert all(f in usage for f in ('--database <PATH>', '--project <DIR>', '--actor <KIND:NAME>', 'human:NAME, agent:NAME or service:NAME')), usage
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    reviewer = Agent(database, 'human:reviewer')
    try:
        names = {tool['name'] for tool in worker.request('tools/list', {})['tools']}
        assert {'add_artifact', 'apply_change', 'attach_git_head', 'claim_work', 'decide_gate', 'explain_work', 'export_mspdi', 'export_plan', 'get_work', 'handoff_work', 'history', 'import_mspdi', 'link_external', 'next_work', 'plan_schema', 'plan_template', 'project_status', 'propose_change', 'ratify_contract', 'reject_work', 'release_work', 'report_blocker', 'report_progress', 'restore_dependency', 'revalidate_basis', 'start_work', 'submit_work', 'unblock_work', 'unlink_external', 'verify_work', 'waive_dependency', 'workspace_list', 'workspace_register'} == names
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
        ]
        # Omitting the actor must never let one caller act as its own independent reviewer.
        before = run_cli(database, 'export')
        for command in (['claim', 'TEST-A'], ['verify', 'TEST-A'], ['ratify', 'TEST-A'], ['plan', 'apply', 'x.json', '--reason', 'r']):
            refused = subprocess.run([str(CLI), '--database', str(database), '--json', *command], cwd=ROOT, capture_output=True, text=True, timeout=15)
            assert refused.returncode != 0 and '--actor' in json.loads(refused.stdout)['error']['message'], (command, refused)
        # Argument errors keep the machine envelope wherever --json appears, and clap text otherwise.
        api_version = worker.call('project_status', {})['api_version']
        for command in (['--bogus', 'status', '--json'], ['--base-revision', 'abc', '--json', 'status'], ['--json', 'workspace']):
            refused = subprocess.run([str(CLI), '--database', str(database), *command], cwd=ROOT, capture_output=True, text=True, timeout=15)
            body = json.loads(refused.stdout)['error']
            assert refused.returncode != 0 and body['code'] == 'invalid_request' and body['api_version'] == api_version, (command, refused)
        human = subprocess.run([str(CLI), '--database', str(database), 'claim', 'TEST-A'], cwd=ROOT, capture_output=True, text=True, timeout=15)
        assert human.returncode != 0 and not human.stdout and '--actor' in human.stderr, human
        assert run_cli(database, 'export') == before
        for tool, arguments, command in missing_pairs:
            agent_error = worker.call(tool, arguments, error='not_found')
            cli_error = run_cli(database, *command, error='not_found')['error']
            # The CLI error object is the MCP tool-error structuredContent, api_version included.
            assert agent_error == cli_error and cli_error['api_version'] == agent_error['api_version']
        missing = reviewer.call('decide_gate', {'decision': 'missing', 'outcome': 'choice', 'base_revision': 0}, error='not_found')
        assert missing['message'] == run_cli(database, 'decide', 'missing', 'choice', error='not_found')['error']['message']
        # Resolving a decision is a human or service act; an agent is refused alike and nothing changes.
        remote = worker.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Proceed', 'base_revision': 0}, error='invalid_command')
        local = run_cli(database, 'decide', 'TEST-GATE', 'Proceed', '--actor', 'agent:parity', error='invalid_command')['error']
        assert remote['message'] == local['message'] and 'resolve a decision' in local['message'], local
        assert run_cli(database, 'export') == before
        context = worker.call('explain_work', {'key': 'TEST-B'})['data']['context']
        assert context['requirements'] and context['decisions'][0]['key'] == 'TEST-GATE'
        gated = worker.call('claim_work', {'key': 'TEST-B', 'base_revision': 0}, error='invalid_command')
        cli_gated = run_cli(database, 'claim', 'TEST-B', error='invalid_command')['error']
        assert gated == cli_gated
        assert gated['details']['unmet'] == worker.call('explain_work', {'key': 'TEST-B'})['data']['gates']['unmet']
        agent_error = worker.call('claim_work', {'key': 'TEST-M1', 'base_revision': 0}, error='invalid_command')
        cli_error = run_cli(database, 'claim', 'TEST-M1', error='invalid_command')['error']
        assert agent_error['message'] == cli_error['message'] and 'not a task' in cli_error['message']
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 50, 'base_revision': 0}, error='invalid_command')
        run_cli(database, 'progress', 'TEST-A', '50', error='invalid_command')
        claim = worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})['data']
        assert claim['resulting_revision'] == 1
        for tool, arguments, command in [
            ('submit_work', {}, ('submit', 'TEST-A', '--actor', 'agent:parity')),
            ('report_progress', {'percent': 10}, ('progress', 'TEST-A', '10', '--actor', 'agent:parity')),
        ]:
            remote = worker.call(tool, {'key': 'TEST-A', 'base_revision': 1, **arguments}, error='invalid_command')
            local = run_cli(database, *command, error='invalid_command')['error']
            assert remote['message'] == local['message'] and 'has not started' in local['message']
        reviewer.call('start_work', {'key': 'TEST-A', 'base_revision': 1}, error='invalid_command')
        started = worker.call('start_work', {'key': 'TEST-A', 'base_revision': 1})['data']
        assert started['command'] == {'Start': {'work': claim['command']['Claim']['work']}}
        events = worker.call('get_work', {'key': 'TEST-A'})['data']['events']
        assert events == {'started_at': started['timestamp']}
        run_cli(database, 'progress', 'TEST-A', '25', '--actor', 'agent:parity')
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 50, 'note': 'Half the acceptance work is implemented', 'base_revision': 3})
        worker.call('report_progress', {'key': 'TEST-A', 'percent': 101, 'base_revision': 4}, error='invalid_command')
        run_cli(database, 'progress', 'TEST-A', '101', '--actor', 'agent:parity', error='invalid_command')
        shown = worker.call('get_work', {'key': 'TEST-A'})['data']
        assert shown == run_cli(database, 'show', 'TEST-A')
        assert shown['progress'] == {'percent_complete': 50.0, 'verified': False}
        assert shown['reported_progress_percent'] == 50 and shown['status'] == 'InProgress'
        summary = worker.call('project_status', {})['data']
        assert summary == run_cli(database, 'status') and summary['progress']['percent_complete'] > 0
        run_cli(database, '--base-revision', '0', 'claim', 'TEST-A', error='revision_conflict')
        worker.call('attach_git_head', {'key': 'TEST-A', 'resource': 'TEST-REPO', 'base_revision': 4})
        run_cli(database, 'block', 'TEST-A', 'Waiting for fixture', '--actor', 'agent:parity')
        assert worker.call('next_work', {})['data']['candidates'] == []
        worker.call('unblock_work', {'key': 'TEST-A', 'base_revision': 6})
        assert worker.call('get_work', {'key': 'TEST-A'})['data']['status'] == 'InProgress'
        worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 7, 'note': 'Acceptance evidence reviewed'})
        worker.call('verify_work', {'key': 'TEST-A', 'base_revision': 8}, error='invalid_command')
        run_cli(database, 'verify', 'TEST-A', '--actor', 'agent:parity', error='invalid_command')
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 8})
        assert worker.call('next_work', {})['data']['candidates'] == []
        reviewer.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Accept the verified input', 'base_revision': 9})
        next_work = worker.call('next_work', {})['data']
        assert next_work == run_cli(database, 'next')
        assert {candidate['work']['key'] for candidate in next_work['candidates']} == {'TEST-B', 'TEST-D'}
        assert worker.call('project_status', {})['data']['revision'] == 10
        verified = worker.call('explain_work', {'key': 'TEST-A'})['data']
        assert verified == run_cli(database, 'explain', 'TEST-A')
        assert verified['progress']['completed_at'] == {'recorded': verified['work']['events']['verified_at']}
        assert set(verified['work']['events']) == {'started_at', 'submitted_at', 'verified_at'}
        decided = next(d for d in run_cli(database, 'export')['decisions'].values() if d['key'] == 'TEST-GATE')
        assert decided['resolved_at']
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
        run_cli(database, 'start', 'TEST-A', '--actor', 'agent:worker')
        run_cli(database, 'submit', 'TEST-A', '--actor', 'agent:worker')
        review = reviewer.call('reject_work', {'key': 'TEST-A', 'reason': 'Missing acceptance evidence', 'base_revision': 4})
        assert review['data']['command']['Reject']['reason'] == 'Missing acceptance evidence'
        detail = worker.call('explain_work', {'key': 'TEST-A'})['data']
        assert detail == run_cli(database, 'explain', 'TEST-A')
        assert detail['work']['status'] == 'InProgress'
        assert detail['work']['last_rejection']['reason'] == 'Missing acceptance evidence'
        assert 'submitted_at' not in detail['work']['events'] and detail['work']['events']['started_at']
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


def dependency_smoke(directory):
    """Soft-edge waiver/restore and non-gating links behave identically through CLI and MCP."""
    database = directory / 'dependencies.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:parity')
    reviewer = Agent(database, 'human:reviewer')
    try:
        plan = run_cli(database, 'export')
        keys = {w['key']: w['id'] for w in plan['work_items'].values()}
        edge = next(d for d in plan['dependencies'] if d['predecessor'] == keys['TEST-A'] and d['successor'] == keys['TEST-B'])
        hard = next(d for d in plan['dependencies'] if d['id'] != edge['id'])
        assert edge['id'] and edge['policy'] == 'Hard' and len({d['id'] for d in plan['dependencies']}) == len(plan['dependencies'])
        edge['policy'], edge['rationale'] = 'Soft', 'B may start from a prototype of A'
        plan['links'] = [{'kind': 'RelatesTo', 'source': keys['TEST-F'], 'target': keys['TEST-A'], 'note': 'shared fixture'}]
        candidate = directory / 'soft.json'
        candidate.write_text(json.dumps(plan))
        preview = worker.call('propose_change', {'plan': plan})['data']
        assert preview == run_cli(database, 'plan', 'diff', str(candidate))
        assert [(c['collection'], c['id'], c['fields']) for c in preview['changes']] == [
            ('dependencies', edge['id'], ['policy', 'rationale']), ('links', None, [])]
        before_links = worker.call('next_work', {'probabilistic': False})['data']
        run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'Allow prototype overlap', '--actor', 'human:reviewer')
        assert worker.call('next_work', {'probabilistic': False})['data'] == before_links
        linked = worker.call('explain_work', {'key': 'TEST-F'})['data']
        assert linked == run_cli(database, 'explain', 'TEST-F') and linked['context']['links'] == plan['links']
        reviewer.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Proceed', 'base_revision': 1})
        enforced = worker.call('explain_work', {'key': 'TEST-B'})['data']
        assert enforced == run_cli(database, 'explain', 'TEST-B') and not enforced['ready']
        [gate] = enforced['gates']['unmet']
        assert (gate['type'], gate['dependency'], gate['policy']) == ('dependency', edge['id'], 'Soft')
        [shown] = [d for d in enforced['context']['dependencies'] if d['id'] == edge['id']]
        assert shown['policy'] == 'Soft' and shown['rationale'] == edge['rationale'] and 'waiver' not in shown
        refusals = [
            (worker, 'waive_dependency', {'dependency': edge['id'], 'reason': 'skip'}, ('waive-dependency', edge['id'], '--reason', 'skip', '--actor', 'agent:parity'), 'invalid_command'),
            (reviewer, 'waive_dependency', {'dependency': hard['id'], 'reason': 'skip'}, ('waive-dependency', hard['id'], '--reason', 'skip'), 'invalid_command'),
            (reviewer, 'waive_dependency', {'dependency': edge['id'], 'reason': ' '}, ('waive-dependency', edge['id'], '--reason', ' '), 'invalid_command'),
            (reviewer, 'restore_dependency', {'dependency': edge['id'], 'reason': 'not waived'}, ('restore-dependency', edge['id'], '--reason', 'not waived'), 'invalid_command'),
            (reviewer, 'waive_dependency', {'dependency': 'not-an-edge', 'reason': 'skip'}, ('waive-dependency', 'not-an-edge', '--reason', 'skip'), 'not_found'),
            (reviewer, 'waive_dependency', {'dependency': edge['id'], 'reason': 'skip', 'base_revision': 0}, ('--base-revision', '0', 'waive-dependency', edge['id'], '--reason', 'skip'), 'revision_conflict'),
        ]
        for actor, tool, arguments, command, code in refusals:
            remote = actor.call(tool, {'base_revision': 2, **arguments}, error=code)
            assert remote['message'] == run_cli(database, *command, error=code)['error']['message'], (tool, arguments)
        assert run_cli(database, 'export')['revision'] == 2
        waived = reviewer.call('waive_dependency', {'dependency': edge['id'], 'reason': 'Prototype of A is sufficient', 'base_revision': 2})['data']
        assert waived['command']['WaiveDependency']['reason'] == 'Prototype of A is sufficient'
        detail = worker.call('explain_work', {'key': 'TEST-B'})['data']
        assert detail == run_cli(database, 'explain', 'TEST-B') and detail['ready'] and detail['gates']['unmet'] == []
        [shown] = [d for d in detail['context']['dependencies'] if d['id'] == edge['id']]
        assert shown['waiver']['actor'] == {'kind': 'Human', 'name': 'reviewer'} and shown['waiver']['reason'] == 'Prototype of A is sufficient'
        assert detail['schedule']['earliest_start_hours'] == 0.0 < enforced['schedule']['earliest_start_hours']
        assert 'TEST-B' in {c['work']['key'] for c in worker.call('next_work', {'probabilistic': False})['data']['candidates']}
        run_cli(database, 'restore-dependency', edge['id'], '--reason', 'Prototype rejected', '--actor', 'service:ci')
        assert worker.call('explain_work', {'key': 'TEST-B'})['data'] == enforced == run_cli(database, 'explain', 'TEST-B')
        history = worker.call('history', {'after_sequence': 2})['data']
        assert history == run_cli(database, 'history', '--after-sequence', '2')
        restored = history['entries'][-1]['operation']
        assert restored['actor'] == {'kind': 'Service', 'name': 'ci'} and restored['timestamp']
        assert restored['command'] == {'RestoreDependency': {'dependency': edge['id'], 'reason': 'Prototype rejected'}}
        owner_endpoint_refusal(database, edge['id'])
    finally:
        worker.close()
        reviewer.close()


def owner_endpoint_refusal(database, edge):
    """A human owning either endpoint cannot waive the edge, identically through CLI and MCP."""
    owner = Agent(database, 'human:owner')
    try:
        run_cli(database, 'claim', 'TEST-A', '--actor', 'human:owner')
        revision = run_cli(database, 'export')['revision']
        arguments = {'dependency': edge, 'reason': 'my own result is enough', 'base_revision': revision}
        remote = owner.call('waive_dependency', arguments, error='invalid_command')
        local = run_cli(database, 'waive-dependency', edge, '--reason', 'my own result is enough', '--actor', 'human:owner',
                        error='invalid_command')
        assert remote['message'] == local['error']['message'] and 'its own work' in remote['message'], remote
        assert run_cli(database, 'export')['revision'] == revision
        # A reviewed change is no side door: the owner may not remove the edge out of its own work.
        current = run_cli(database, 'export')
        current['dependencies'] = [d for d in current['dependencies'] if d['id'] != edge]
        candidate = database.parent / 'own-relaxation.json'
        candidate.write_text(json.dumps(current))
        arguments = {'plan': current, 'reason': 'my result no longer gates B', 'base_revision': revision}
        remote = owner.call('apply_change', arguments, error='invalid_command')
        local = run_cli(database, 'plan', 'apply', str(candidate), '--reason', 'my result no longer gates B',
                        '--actor', 'human:owner', error='invalid_command')['error']
        assert remote['message'] == local['message'] and remote['details'] == local['details'], (remote, local)
        assert remote['details'] == {'work': 'TEST-A', 'relaxed': {
            'type': 'dependency', 'dependency': edge, 'predecessor': 'TEST-A', 'successor': 'TEST-B',
            'relaxation': 'removed'}}, remote
        after = run_cli(database, 'export')
        assert after['revision'] == revision and any(d['id'] == edge for d in after['dependencies'])
    finally:
        owner.close()


def timing_plan(kind, lag, legacy=False):
    """TEST-A -> TEST-B under one relation and lag; `legacy` marks A verified without event times."""
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    keys = {w['key']: w['id'] for w in plan['work_items'].values()}
    plan['decisions'] = {}
    for edge in plan['dependencies']:
        if edge['predecessor'] == keys['TEST-A'] and edge['successor'] == keys['TEST-B']:
            edge['kind'], edge['lag_hours'] = kind, lag
    if legacy:
        task = plan['work_items'][keys['TEST-A']]
        task['status'], task['owner'] = 'Verified', {'kind': 'Agent', 'name': 'legacy'}
    return plan


def same_at_nearby_clocks(left, right, key=''):
    """Equal views taken at two clock readings: forecast hours measured from each reading while a
    lag elapses may differ by the time between the calls, and nothing else may differ."""
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(same_at_nearby_clocks(left[k], right[k], k) for k in left)
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(same_at_nearby_clocks(a, b, key) for a, b in zip(left, right))
    if key.endswith('_hours') and isinstance(left, float) and isinstance(right, float):
        return abs(left - right) < 0.01
    return left == right


def timing_smoke(directory):
    """Elapsed-lag gates, start events and unknown legacy times read identically through CLI and MCP."""
    fixture = directory / 'lag.json'
    fixture.write_text(json.dumps(timing_plan('StartStart', 24.0)))
    database = directory / 'lag.sqlite'
    run_cli(database, 'import', str(fixture))
    worker = Agent(database, 'agent:timing')
    try:
        worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})
        awaiting = worker.call('explain_work', {'key': 'TEST-B'})['data']
        assert awaiting == run_cli(database, 'explain', 'TEST-B')
        [gate] = awaiting['gates']['unmet']
        assert (gate['relation'], gate['requires'], gate['release']) == ('StartStart', 'start', {'state': 'awaiting_event'})
        started = run_cli(database, 'start', 'TEST-A', '--actor', 'agent:timing')
        elapsing = worker.call('explain_work', {'key': 'TEST-B'})['data']
        assert same_at_nearby_clocks(elapsing, run_cli(database, 'explain', 'TEST-B')) and not elapsing['ready']
        # The forecast opens B with the gate: 24h after A's recorded start, not 24h after now.
        assert 23.9 < elapsing['schedule']['earliest_start_hours'] < 24.0, elapsing['schedule']
        [gate] = elapsing['gates']['unmet']
        assert gate['release']['state'] == 'elapsing' and gate['release']['event_at'] == started['timestamp']
        assert elapsing['transitions']['submit']['unmet'][0]['type'] == 'lifecycle'
        remote = worker.call('claim_work', {'key': 'TEST-B', 'base_revision': 2}, error='invalid_command')
        local = run_cli(database, 'claim', 'TEST-B', '--actor', 'agent:timing', error='invalid_command')['error']
        assert remote['details'] == local['details']
        assert local['details']['unmet'][0]['release']['opens_at'] == gate['release']['opens_at']
        assert worker.call('next_work', {'probabilistic': False})['data'] == run_cli(database, 'next', '--deterministic-only')
    finally:
        worker.close()
    cases = [('legacy-zero', 'FinishStart', 0.0, True), ('legacy-positive', 'FinishStart', 1.0, False),
             ('lead', 'FinishFinish', -8.0, True)]
    for name, kind, lag, ready in cases:
        fixture = directory / f'{name}.json'
        fixture.write_text(json.dumps(timing_plan(kind, lag, legacy=name.startswith('legacy'))))
        database = directory / f'{name}.sqlite'
        run_cli(database, 'import', str(fixture))
        agent = Agent(database, 'agent:timing')
        try:
            detail = agent.call('explain_work', {'key': 'TEST-B'})['data']
            assert detail == run_cli(database, 'explain', 'TEST-B') and detail['ready'] == ready, name
            if name == 'legacy-positive':
                assert detail['gates']['unmet'][0]['release'] == {'state': 'unrecorded_event_time'}
                assert any('was not recorded' in reason for reason in detail['why_now'])
            if name == 'lead':
                assert detail['transitions']['submit']['unmet'][-1]['release'] == {'state': 'awaiting_event'}
                assert any('schedule only' in reason for reason in detail['why_now'])
        finally:
            agent.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-agent-') as directory:
        smoke(Path(directory) / 'plan.sqlite')
        review_smoke(Path(directory))
        scope_smoke(Path(directory))
        dependency_smoke(Path(directory))
        timing_smoke(Path(directory))
        # Conditional-work parity lives in its own module, which reuses this module's helpers.
        from smoke_conditional import smoke as conditional_smoke
        conditional_smoke(directory)
        from smoke_provisional import provisional_smoke
        provisional_smoke(Path(directory))
        from smoke_ownership import ownership_smoke
        ownership_smoke(Path(directory))
    print('PASS: CLI/MCP query parity, revision conflicts, evidence, blockers, gates and independent verification')
    print('PASS: scoped next parity, outside-scope visibility, limits and unknown scope keys without state change')
    print('PASS: CLI/MCP dependency identity, soft-edge waiver/restore, refusals and non-gating links')
    print('PASS: CLI/MCP start events, elapsed-lag gates, unknown legacy event times and lead explanations')
    print('PASS: CLI/MCP release and handoff parity, refusals, evidence-author and holder independence, history after restart')
    print('PASS: CLI/MCP conditional work: options, applicability gates, branch joins, excluded packages, scenarios and reviewed choice changes')
