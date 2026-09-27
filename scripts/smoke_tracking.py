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
    worker.call('start_work', {'key': 'TEST-A', 'base_revision': 5})
    worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 6})
    run_cli(database, 'link-external', 'TEST-A', *flags(PULL), '--observed', 'merged', '--actor', 'agent:parity')
    shown = worker.call('get_work', {'key': 'TEST-A'})['data']
    assert shown == run_cli(database, 'show', 'TEST-A')
    assert shown['status'] == 'Submitted' and shown['artifact_ids'] == [], 'a merged PR is neither evidence nor verification'
    worker.call('verify_work', {'key': 'TEST-A', 'base_revision': 8}, error='invalid_command')
    reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 8})
    history = worker.call('history', {})['data']
    assert history == run_cli(database, 'history')
    commands = [next(iter(e['operation']['command'])) for e in history['entries']]
    assert commands == ['LinkExternal', 'LinkExternal', 'LinkExternal', 'UnlinkExternal', 'Claim', 'Start', 'Submit', 'LinkExternal', 'Verify']


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
        # Linear folds key and workspace case; GitHub/Forgejo issues and pull requests share numbers.
        linear = {'provider': 'Linear', 'instance': 'linear.app', 'namespace': 'acme', 'kind': 'Issue', 'external_id': 'ENG-1'}
        worker.call('link_external', {'key': 'TEST-A', 'identity': linear, 'base_revision': 1})
        variant = {**linear, 'namespace': 'Acme', 'external_id': 'eng-1'}
        refused(database, worker, 'link_external', {'key': 'TEST-B', 'identity': variant, 'base_revision': 2},
                ['link-external', 'TEST-B', *flags(variant)], 'tracking_conflict')
        issue = {'provider': 'GitHub', 'instance': 'github.com', 'namespace': 'o/r', 'kind': 'Issue', 'external_id': '5'}
        worker.call('link_external', {'key': 'TEST-A', 'identity': issue, 'base_revision': 2})
        pull = {**issue, 'kind': 'pulls', 'namespace': 'O/R.git', 'external_id': '005'}
        refused(database, worker, 'link_external', {'key': 'TEST-C', 'identity': {**pull, 'kind': 'PullRequest'}, 'base_revision': 3},
                ['link-external', 'TEST-C', *flags(pull)], 'tracking_conflict')
        assert len(run_cli(database, 'export')['external_references']) == 3
    finally:
        worker.close()



def forge_family(directory):
    """Forgejo and Gitea number one instance's objects alike, so either family names one object."""
    database = directory / 'forge-family.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:family')
    try:
        forgejo = {'provider': 'Forgejo', 'instance': 'codeberg.org', 'namespace': 'ops/dpm', 'kind': 'Issue', 'external_id': '6'}
        worker.call('link_external', {'key': 'TEST-A', 'identity': forgejo, 'base_revision': 0})
        gitea = {**forgejo, 'provider': 'Gitea'}
        message = refused(database, worker, 'link_external', {'key': 'TEST-B', 'identity': gitea, 'base_revision': 1},
                          ['link-external', 'TEST-B', *flags(gitea)], 'tracking_conflict')
        assert 'already tracked by TEST-A' in message, message
        run_cli(database, 'link-external', 'TEST-B', *flags(gitea), '--role', 'relates', '--actor', 'agent:family')
        worker.call('link_external', {'key': 'TEST-C', 'identity': {**gitea, 'instance': 'gitea.com'}, 'base_revision': 2})
        stored = run_cli(database, 'export')['external_references'].values()
        assert sorted((r['identity']['provider'], r['identity']['instance'], len(r['links'])) for r in stored) == [
            ('Forgejo', 'codeberg.org', 2), ('Gitea', 'gitea.com', 1)], stored
        leaked = {**forgejo, 'external_id': '8'}
        for label, url in [('see codeberg.org/ops/dpm?%74oken=abc', None), ('tok@codeberg.org/ops/dpm', None),
                           ('Issue 8', 'https://codeberg.org/ops/dpm/issues/8?API%5FKEY=abc')]:
            arguments = {'key': 'TEST-D', 'identity': leaked, 'label': label, 'base_revision': 3}
            cli = ['link-external', 'TEST-D', *flags(leaked), '--label', label]
            if url:
                arguments['url'] = url
                cli += ['--url', url]
            message = refused(database, worker, 'link_external', arguments, cli, 'invalid_command')
            assert 'credentials' in message or 'secret' in message, message
    finally:
        worker.close()


def issue_then_pull(directory):
    """A shared-number object first recorded as an issue becomes a pull request when relinked as one."""
    database = directory / 'issue-then-pull.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:upgrade')
    try:
        issue = {'provider': 'GitHub', 'instance': 'github.com', 'namespace': 'o/r', 'kind': 'Issue', 'external_id': '5'}
        worker.call('link_external', {'key': 'TEST-A', 'identity': issue, 'base_revision': 0})
        pulls = {**issue, 'kind': 'pulls'}
        run_cli(database, 'link-external', 'TEST-A', *flags(pulls), '--observed', 'merged', '--actor', 'agent:upgrade')
        explained = worker.call('explain_work', {'key': 'TEST-A'})['data']
        assert explained == run_cli(database, 'explain', 'TEST-A')
        [reference] = explained['context']['external_references']
        assert reference['identity'] == {**issue, 'kind': 'PullRequest'}, reference
        assert reference['observation']['state'] == 'Merged' and len(reference['links']) == 1, reference
        closed = {'key': 'TEST-B', 'identity': issue, 'role': 'Relates', 'observed': 'Closed', 'base_revision': 2}
        message = refused(database, worker, 'link_external', closed,
                          ['link-external', 'TEST-B', *flags(issue), '--role', 'relates', '--observed', 'closed'],
                          'invalid_command')
        assert 'Merged' in message, 'a merged observation is final: ' + message
        worker.call('link_external', {'key': 'TEST-B', 'identity': issue, 'role': 'Relates', 'base_revision': 2})
        [stored] = run_cli(database, 'export')['external_references'].values()
        assert stored['observation']['state'] == 'Merged', stored
        assert stored['identity']['kind'] == 'PullRequest', 'an issue-kind link never downgrades the record'
        pull = {**issue, 'kind': 'PullRequest'}
        refused(database, worker, 'link_external', {'key': 'TEST-A', 'identity': pull, 'base_revision': 3},
                ['link-external', 'TEST-A', *flags(pull)], 'invalid_command')
        history = worker.call('history', {})['data']
        assert history == run_cli(database, 'history')
        upgrade = history['entries'][1]['operation']['command']['LinkExternal']
        assert upgrade['identity']['kind'] == 'PullRequest' and upgrade['observed'] == 'Merged', upgrade
    finally:
        worker.close()

def revision(database):
    return run_cli(database, 'export')['revision']


def mcp_identity(identity):
    """JSON callers name a kind outside Issue/PullRequest as an Other kind."""
    kind = identity['kind']
    return {**identity, 'kind': kind if kind in ('Issue', 'PullRequest') else {'Other': kind}}


def one_decision_per_rule(directory):
    """Each rule decides once for both adapters: credentials, kind aliases, ports, namespaces, kind changes."""
    database = directory / 'rules.sqlite'
    run_cli(database, 'import', str(ROOT / 'tests/support/execution-plan.json'))
    worker = Agent(database, 'agent:rules')
    try:
        issue = {'provider': 'GitHub', 'instance': 'github.com', 'namespace': 'o/r', 'kind': 'Issue', 'external_id': '6'}
        leaked = {**issue, 'external_id': '8'}
        cases = [(label, None) for label in ['//ghp_abc@github.com/o/r', 'https:/tok@github.com/o/r',
                                             'https:\\\\tok@github.com\\o\\r', 'user:ghp_secret@github.com',
                                             'ghp_abc@[2001:db8::1]/o/r']]
        cases += [('Issue 8', url) for url in ['https://github.com/o/r/issues/8;token=abc',
                                               'https://github.com/o/r/issues/8/token=abc',
                                               'https://github.com/o/r/issues/8?accessToken=abc']]
        for label, url in cases:
            arguments = {'key': 'TEST-D', 'identity': leaked, 'label': label, 'base_revision': 0}
            cli = ['link-external', 'TEST-D', *flags(leaked), '--label', label]
            if url:
                arguments['url'] = url
                cli += ['--url', url]
            message = refused(database, worker, 'link_external', arguments, cli, 'invalid_command')
            assert 'credentials' in message or 'secret' in message, (label, url, message)
        label = 'max_tokens=4096 secret_santa=on see #token-refresh; clone git@github.com:o/r.git'
        worker.call('link_external', {'key': 'TEST-A', 'identity': issue, 'label': label, 'base_revision': 0,
                                      'url': 'https://github.com:0443/o/r/issues/6#api-key-setup'})
        variants = [{**issue, 'kind': kind} for kind in ['pull_requests', 'pull-requests', 'prs', 'pullrequests', 'discussion']]
        variants += [{**issue, 'instance': 'github.com:0443'}, {**issue, 'namespace': './o/r'}, {**issue, 'namespace': 'o//r'}]
        for variant in variants:
            refused(database, worker, 'link_external', {'key': 'TEST-B', 'identity': mcp_identity(variant), 'base_revision': 1},
                    ['link-external', 'TEST-B', *flags(variant)], 'tracking_conflict')
        for variant in [{**issue, 'kind': 'weird'}, {**issue, 'namespace': 'o/../r'}, {**issue, 'instance': 'github.com:0'}]:
            refused(database, worker, 'link_external', {'key': 'TEST-B', 'identity': mcp_identity(variant), 'base_revision': 1},
                    ['link-external', 'TEST-B', *flags(variant)], 'invalid_command')
        pull = {**issue, 'kind': 'PullRequest'}
        worker.call('link_external', {'key': 'TEST-B', 'identity': pull, 'role': 'Relates', 'base_revision': 1})
        [stored] = run_cli(database, 'export')['external_references'].values()
        assert stored['identity']['kind'] == 'Issue' and len(stored['links']) == 2, 'context links never refine the kind'
        run_cli(database, 'link-external', 'TEST-C', *flags({**pull, 'external_id': '7'}), '--actor', 'agent:rules')
        current = run_cli(database, 'export')
        proposal = copy.deepcopy(current)
        for reference in proposal['external_references'].values():
            if reference['identity']['kind'] == 'PullRequest':
                reference['identity']['kind'] = 'Issue'
        candidate = directory / 'downgrade.json'
        candidate.write_text(json.dumps(proposal))
        remote = worker.call('propose_change', {'plan': proposal}, error='invalid_command')
        local = run_cli(database, 'plan', 'diff', str(candidate), error='invalid_command')['error']
        assert remote['message'] == local['message'] and 'kind' in remote['message'], (remote, local)
        assert revision(database) == current['revision']
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
            forge_family(directory)
            issue_then_pull(directory)
            one_decision_per_rule(directory)
        finally:
            worker.close()
            reviewer.close()
        reopened = Agent(database, 'agent:reader')
        try:
            assert reopened.call('export_plan', {})['data'] == final
        finally:
            reopened.close()
    print('PASS: provider-scoped identities without collisions, exclusive tracking, atomic stale/credential/missing rejections, unlink without graph change, observations never verify, CLI/MCP parity, per-provider key normalization, one Forgejo/Gitea object family per instance, scheme-less and percent-encoded credential rejection, one credential detector for labels and URLs, issue-to-pull-request kind upgrade by the owner only, final merged observations, kind/port/namespace spellings with one owner, reviewed changes never downgrade kinds, reviewed rename and export/import round trip')
