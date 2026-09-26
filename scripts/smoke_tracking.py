#!/usr/bin/env python3
"""Check provider-scoped external tracking links through the real CLI and agent transport."""
import copy
import json
import re
import tempfile
from pathlib import Path

from smoke_agent import Agent, ROOT, run_cli

ALPHA = {'provider': 'Forgejo', 'instance': 'git.alpha.example', 'namespace': 'ops/dpm', 'kind': 'Issue', 'external_id': '42'}
BETA = {**ALPHA, 'instance': 'git.beta.example:3000'}
HOSTED = {**ALPHA, 'provider': 'GitHub', 'instance': 'github.com'}
PULL = {**HOSTED, 'kind': 'PullRequest', 'external_id': '7'}


def flags(identity):
    args = ['--provider', identity['provider'], '--instance', identity['instance'],
            '--kind', identity['kind'], '--id', identity['external_id']]
    return args + (['--namespace', identity['namespace']] if 'namespace' in identity else [])


def without_uuids(message):
    return re.sub(r'[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}', 'ID', message)


def refused(database, worker, tool, arguments, cli_args, code):
    """Both adapters reject with the same code and message, and the store is unchanged."""
    before = run_cli(database, 'export')
    remote = worker.call(tool, arguments, error=code)
    local = run_cli(database, *cli_args, error=code)['error']
    assert without_uuids(remote['message']) == without_uuids(local['message']), (remote, local)
    assert run_cli(database, 'export') == before
    return remote['message']


def graph(plan):
    return {k: plan[k] for k in ('work_items', 'dependencies', 'artifacts', 'decisions', 'revision')}


def views(database, worker):
    status = worker.call('project_status', {'probabilistic': False})['data']
    assert status == run_cli(database, 'status', '--no-simulation')
    status.pop('revision')
    ranked = worker.call('next_work', {'probabilistic': False})['data']
    assert ranked == run_cli(database, 'next', '--deterministic-only')
    return status, ranked


def link_and_refuse(database, worker):
    baseline = views(database, worker)
    run_cli(database, 'link-external', 'TEST-A', *flags({**ALPHA, 'instance': 'Git.Alpha.Example'}),
            '--url', 'https://git.alpha.example/ops/dpm/issues/42', '--actor', 'agent:parity')
    worker.call('link_external', {'key': 'TEST-B', 'identity': BETA, 'base_revision': 1})
    worker.call('link_external', {'key': 'TEST-C', 'identity': HOSTED, 'observed': 'Closed', 'base_revision': 2})
    assert views(database, worker) == baseline, 'links never change readiness, ranking or schedule'
    plan = run_cli(database, 'export')
    assert len(plan['external_references']) == 3, 'equal numbers on other instances never collide'
    for key in ['TEST-A', 'TEST-B', 'TEST-C']:
        explained = worker.call('explain_work', {'key': key})['data']
        assert explained == run_cli(database, 'explain', key)
        [reference] = explained['context']['external_references']
        assert reference['links'] == [{'work': explained['work']['id'], 'role': 'Tracks'}]
        assert explained['context']['artifacts'] == [] and explained['work']['status'] == 'Planned'
    message = refused(database, worker, 'link_external', {'key': 'TEST-D', 'identity': ALPHA, 'base_revision': 3},
                      ['link-external', 'TEST-D', *flags(ALPHA)], 'tracking_conflict')
    assert 'already tracked by TEST-A' in message
    refused(database, worker, 'link_external', {'key': 'TEST-D', 'identity': {**ALPHA, 'external_id': '9'}, 'base_revision': 2},
            ['--base-revision', '2', 'link-external', 'TEST-D', *flags({**ALPHA, 'external_id': '9'})], 'revision_conflict')
    for url in ['https://token@git.alpha.example/ops/dpm/issues/9', 'https://git.alpha.example/ops/dpm/issues/9?access_token=x']:
        message = refused(database, worker, 'link_external', {'key': 'TEST-D', 'identity': {**ALPHA, 'external_id': '9'}, 'url': url, 'base_revision': 3},
                          ['link-external', 'TEST-D', *flags({**ALPHA, 'external_id': '9'}), '--url', url], 'invalid_command')
        assert 'credentials' in message or 'secret' in message, message
    refused(database, worker, 'link_external', {'key': 'NOPE', 'identity': ALPHA, 'base_revision': 3},
            ['link-external', 'NOPE', *flags(ALPHA)], 'not_found')
    refused(database, worker, 'unlink_external', {'key': 'TEST-A', 'identity': {**ALPHA, 'external_id': '9'}, 'base_revision': 3},
            ['unlink-external', 'TEST-A', *flags({**ALPHA, 'external_id': '9'})], 'not_found')
    refused(database, worker, 'unlink_external', {'key': 'TEST-D', 'identity': ALPHA, 'base_revision': 3},
            ['unlink-external', 'TEST-D', *flags(ALPHA)], 'not_found')
    refused(database, worker, 'unlink_external', {'key': 'TEST-A', 'identity': ALPHA, 'base_revision': 2},
            ['--base-revision', '2', 'unlink-external', 'TEST-A', *flags(ALPHA)], 'revision_conflict')


def unlink_and_merge(database, worker, reviewer):
    before = run_cli(database, 'export')
    worker.call('unlink_external', {'key': 'TEST-A', 'identity': ALPHA, 'base_revision': 3})
    after = run_cli(database, 'export')
    assert {k: v for k, v in graph(after).items() if k != 'revision'} == {k: v for k, v in graph(before).items() if k != 'revision'}
    assert len(after['external_references']) == 2 and after['revision'] == 4
    worker.call('claim_work', {'key': 'TEST-A', 'base_revision': 4})
    worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 5})
    run_cli(database, 'link-external', 'TEST-A', *flags(PULL), '--observed', 'merged', '--actor', 'agent:parity')
    shown = worker.call('get_work', {'key': 'TEST-A'})['data']
    assert shown == run_cli(database, 'show', 'TEST-A')
    assert shown['status'] == 'Submitted' and shown['artifact_ids'] == [], 'a merged PR is neither evidence nor verification'
    worker.call('verify_work', {'key': 'TEST-A', 'base_revision': 7}, error='invalid_command')
    reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 7})
    history = worker.call('history', {})['data']
    assert history == run_cli(database, 'history')
    commands = [next(iter(e['operation']['command'])) for e in history['entries']]
    assert commands == ['LinkExternal', 'LinkExternal', 'LinkExternal', 'UnlinkExternal', 'Claim', 'Submit', 'LinkExternal', 'Verify']


def review_and_round_trip(database, directory, worker, reviewer):
    current = run_cli(database, 'export')
    proposal = copy.deepcopy(current)
    reference = next(r for r in proposal['external_references'].values() if r['identity'] == HOSTED)
    reference['label'] = 'Moved repository issue'
    reference['identity']['namespace'] = 'platform/dpm'
    candidate = directory / 'tracking-proposal.json'
    candidate.write_text(json.dumps(proposal))
    preview = worker.call('propose_change', {'plan': proposal})['data']
    assert preview == run_cli(database, 'plan', 'diff', str(candidate))
    [change] = preview['changes']
    assert (change['collection'], change['fields']) == ('external_references', ['identity', 'label'])
    bad = copy.deepcopy(proposal)
    next(iter(bad['external_references'].values()))['url'] = 'https://user:pw@github.com/x'
    candidate.write_text(json.dumps(bad))
    remote = worker.call('propose_change', {'plan': bad}, error='invalid_command')
    assert remote['message'] == run_cli(database, 'plan', 'diff', str(candidate), error='invalid_command')['error']['message']
    reviewer.call('apply_change', {'plan': proposal, 'reason': 'Repository moved', 'base_revision': current['revision']})
    moved = worker.call('explain_work', {'key': 'TEST-C'})['data']['context']['external_references']
    assert moved[0]['id'] == reference['id'] and moved[0]['identity']['namespace'] == 'platform/dpm'
    exported = run_cli(database, 'export')
    copy_path = directory / 'imported.sqlite'
    exported_file = directory / 'exported.json'
    exported_file.write_text(json.dumps(exported))
    run_cli(copy_path, 'import', str(exported_file))
    assert run_cli(copy_path, 'export') == exported, 'references survive export/import'
    return exported


def jira_identity(directory):
    """A Jira key names its project and is unique per instance: no namespace, and case never splits it."""
    database = directory / 'jira.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:jira')
    try:
        jira = {'provider': 'Jira', 'instance': 'acme.atlassian.net', 'kind': 'Issue', 'external_id': 'PROJ-1'}
        scoped = {**jira, 'namespace': 'PROJ'}
        message = refused(database, worker, 'link_external', {'key': 'TEST-D', 'identity': scoped, 'base_revision': 0},
                          ['link-external', 'TEST-D', *flags(scoped)], 'invalid_command')
        assert 'omit the namespace' in message, message
        worker.call('link_external', {'key': 'TEST-D', 'identity': {**jira, 'external_id': 'proj-1'}, 'base_revision': 0})
        stored = run_cli(database, 'export')['external_references']
        assert [r['identity']['external_id'] for r in stored.values()] == ['PROJ-1'], stored
        lower = {**jira, 'external_id': 'proj-1'}
        refused(database, worker, 'link_external', {'key': 'TEST-E', 'identity': lower, 'base_revision': 1},
                ['link-external', 'TEST-E', *flags(lower)], 'tracking_conflict')
    finally:
        worker.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-tracking-') as temporary:
        directory = Path(temporary)
        database = directory / 'tracking.sqlite'
        run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
        worker = Agent(database, 'agent:parity')
        reviewer = Agent(database, 'human:reviewer')
        try:
            names = {tool['name'] for tool in worker.request('tools/list', {})['tools']}
            assert {'link_external', 'unlink_external'} <= names
            link_and_refuse(database, worker)
            unlink_and_merge(database, worker, reviewer)
            final = review_and_round_trip(database, directory, worker, reviewer)
            jira_identity(directory)
        finally:
            worker.close()
            reviewer.close()
        reopened = Agent(database, 'agent:reader')
        try:
            assert reopened.call('export_plan', {})['data'] == final
        finally:
            reopened.close()
    print('PASS: provider-scoped identities without collisions, exclusive tracking, atomic stale/credential/missing rejections, unlink without graph change, observations never verify, CLI/MCP parity, Jira key normalization, reviewed rename and export/import round trip')
