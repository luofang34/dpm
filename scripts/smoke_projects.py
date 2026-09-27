#!/usr/bin/env python3
"""Exercise project discovery, file previews and normal SQLite projects through real adapters."""
import json
import os
import sqlite3
import subprocess
import tempfile
from pathlib import Path

from smoke_agent import Agent, CLI, MCP, ROOT, with_actor


def cli(cwd, *arguments, error=None):
    result = subprocess.run([str(CLI), '--json', *map(str, with_actor(arguments))], cwd=cwd,
                            capture_output=True, text=True, timeout=30)
    value = json.loads(result.stdout)
    if error:
        assert result.returncode != 0 and value['error']['code'] == error, (arguments, value, result.stderr)
    else:
        assert result.returncode == 0, (arguments, value, result.stderr)
    return value


def preview(directory):
    before = (ROOT / 'examples/self-host/dpm-alpha.json').read_bytes()
    runtime = {p.name for p in (ROOT / '.dpm').iterdir()}
    nested = ROOT / 'crates/dpm-model/src'
    assert cli(nested, 'status', '--no-simulation')['revision'] == 0
    assert cli(nested, 'next', '--deterministic-only')['candidates'] == []
    assert cli(directory, '--project', ROOT, 'export') == json.loads(before)
    assert cli(nested)['ready'] == 0
    actor = Agent(None, 'agent:preview', cwd=nested)
    try:
        assert actor.call('get_work', {'key': 'MVP-10'})['data'] == cli(nested, 'show', 'MVP-10')
        for tool, args, command in [
            ('claim_work', {'key': 'MVP-10'}, ('claim', 'MVP-10')),
            ('decide_gate', {'decision': 'DEC-EXECUTE', 'outcome': 'start'}, ('decide', 'DEC-EXECUTE', 'start')),
            ('attach_git_head', {'key': 'MVP-10'}, ('attach-git-head', 'MVP-10')),
        ]:
            remote = actor.call(tool, {**args, 'base_revision': 0}, error='read_only_project')
            local = cli(nested, *command, error='read_only_project')['error']
            assert remote['message'] == local['message']
    finally:
        actor.close()
    assert (ROOT / 'examples/self-host/dpm-alpha.json').read_bytes() == before
    assert {p.name for p in (ROOT / '.dpm').iterdir()} == runtime


def normal(directory):
    root = directory / 'normal'
    root.mkdir()
    cli(root, 'status', error='project_not_found')
    assert not (root / '.dpm').exists()
    # Synthetic execution input is independent of the unstarted self-host roadmap.
    initialized = cli(root, 'import', ROOT / 'tests/support/execution-plan.json')
    database = root / '.dpm/state.sqlite'
    assert Path(initialized['database']).resolve() == database.resolve()
    child = root / 'src/nested'
    child.mkdir(parents=True)
    actor = Agent(None, 'agent:discovery', cwd=child)
    try:
        assert actor.call('project_status', {'probabilistic': False})['data'] == cli(child, 'status', '--no-simulation')
        actor.call('claim_work', {'key': 'TEST-A', 'base_revision': 0})
        actor.call('start_work', {'key': 'TEST-A', 'base_revision': 1})
        cli(child, 'submit', 'TEST-A', '--actor', 'agent:discovery')
        cli(child, 'verify', 'TEST-A', '--actor', 'human:reviewer')
        assert actor.call('get_work', {'key': 'TEST-A'})['data']['status'] == 'Verified'
    finally:
        actor.close()
    cli(root, 'init', error='project_exists')
    with sqlite3.connect(database) as connection:
        assert connection.execute('select count(*) from operations').fetchone()[0] == 4
    malformed = child / '.dpm'
    malformed.mkdir()
    (malformed / 'project.toml').write_text('version = 99\ndatabase = "missing.sqlite"\n')
    cli(child, 'status', error='project_configuration')
    assert cli(child, '--database', database, 'status', '--no-simulation')['revision'] == 4
    assert cli(child, '--project', root, 'status', '--no-simulation')['revision'] == 4
    foreign = root / 'separate-git'
    foreign.mkdir()
    (foreign / '.git').write_text('gitdir: elsewhere\n')
    cli(foreign, 'status', error='project_not_found')
    empty = directory / 'empty'
    empty.mkdir()
    cli(directory, '--project', empty, 'init', 'My project')
    assert cli(empty, 'status', '--no-simulation')['total_work'] == 0


def git_evidence(directory):
    root = directory / 'evidence'
    root.mkdir()
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=root, text=True).strip()
    git('init', '--quiet')
    git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--quiet', '--allow-empty', '-m', 'test evidence')
    head = git('rev-parse', 'HEAD')
    cli(root, 'import', ROOT / 'tests/support/execution-plan.json')
    cli(directory, '--project', root, 'attach-git-head', 'TEST-A', error='invalid_request')
    cli(directory, '--project', root, 'attach-git-head', 'TEST-A', '--resource', 'UNKNOWN', error='invalid_request')
    cli(directory, '--project', root, 'attach-git-head', 'TEST-A', '--resource', 'TEST-REPO')
    actor = Agent(None, 'agent:evidence', cwd=directory, project=root)
    try:
        actor.call('attach_git_head', {'key': 'TEST-A', 'resource': 'TEST-REPO', 'base_revision': 1})
    finally:
        actor.close()
    exported = cli(root, 'export')
    assert len(exported['artifacts']) == 2
    assert all(a['metadata']['commit'] == head for a in exported['artifacts'].values())
    resource = next(iter(exported['resources']))
    assert all(a['metadata']['resource_id'] == resource and a['uri'] == f'git:resource:{resource}@{head}' for a in exported['artifacts'].values())


def bindings(directory):
    database = directory / 'shared.sqlite'
    cli(directory, '--database', database, 'import', ROOT / 'tests/support/execution-plan.json')
    env = {**os.environ, 'DPM_CONFIG_DIR': str(directory / 'config')}
    def local(*args, error=None):
        result = subprocess.run([str(CLI), '--json', *map(str, with_actor(args))], cwd=directory, env=env, capture_output=True, text=True, timeout=30)
        value = json.loads(result.stdout)
        if error:
            assert result.returncode and value['error']['code'] == error, value
        else:
            assert result.returncode == 0, value
        return value
    unbound = directory / 'unbound-checkout'
    (unbound / '.dpm').mkdir(parents=True)
    workspace = local('--database', database, 'export')['workspace']['id']
    (unbound / '.dpm/project.toml').write_text(f"version = 2\nworkspace = '{workspace}'\nresource = 'TEST-REPO'\n")
    missing = local('--project', unbound, 'status', error='workspace_not_bound')['error']['message']
    assert workspace in missing and 'workspace register' in missing
    started = subprocess.run([str(MCP), '--project', str(unbound), '--actor', 'agent:unbound'], cwd=directory, env=env,
                             stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
    assert started.returncode != 0 and f'workspace_not_bound: {missing}' in started.stderr, started.stderr
    actor = Agent(None, 'human:device', project=ROOT, env=env)
    try:
        assert actor.call('workspace_list', {})['data'] == local('workspace', 'list') == []
        assert not (directory / 'config').exists()
        registered = actor.call('workspace_register', {'database': str(database)})['data']
        assert registered == local('workspace', 'register', '--database', database)
        assert actor.call('workspace_list', {})['data'] == local('workspace', 'list')
    finally:
        actor.close()
    for name in ['repository-one', 'documents']:
        root = directory / name
        (root / '.dpm').mkdir(parents=True)
        (root / '.dpm/project.toml').write_text(f"version = 2\nworkspace = '{registered['workspace']}'\nresource = 'TEST-REPO'\n")
    first, second = directory / 'repository-one', directory / 'documents'
    local('--project', first, 'claim', 'TEST-A', '--actor', 'agent:shared')
    assert local('--project', second, 'show', 'TEST-A')['owner']['name'] == 'shared'
    local('--project', second, '--base-revision', '0', 'claim', 'TEST-A', error='revision_conflict')
    exported = local('--project', first, 'export')
    moved = directory / 'moved/renamed-checkout'
    moved.parent.mkdir()
    first.rename(moved)
    assert local('--project', moved, 'export') == exported
    assert local('--project', moved, 'workspace', 'list') == local('workspace', 'list')
    assert len(local('workspace', 'list')) == 1


def stale_bindings(directory):
    """A path holds one store: a second identity is refused, and stale bindings are named, not raw storage errors."""
    env = {**os.environ, 'DPM_CONFIG_DIR': str(directory / 'stale-config')}
    def local(*args, error=None):
        result = subprocess.run([str(CLI), '--json', *map(str, with_actor(args))], cwd=directory, env=env,
                                capture_output=True, text=True, timeout=30)
        value = json.loads(result.stdout)
        assert (result.returncode != 0 and value['error']['code'] == error) if error else result.returncode == 0, (args, value)
        return value
    def mcp_start(project):
        return subprocess.run([str(MCP), '--project', str(project), '--actor', 'agent:stale'], cwd=directory, env=env,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
    store = directory / 'stale.sqlite'
    local('--database', store, 'import', ROOT / 'tests/support/execution-plan.json')
    first = local('workspace', 'register', '--database', store)['workspace']
    checkout = directory / 'stale-checkout'
    (checkout / '.dpm').mkdir(parents=True)
    (checkout / '.dpm/project.toml').write_text(f"version = 2\nworkspace = '{first}'\n")
    store.unlink()
    local('--database', store, 'init', 'Unrelated')
    second = local('--database', store, 'export')['workspace']['id']
    refused = local('workspace', 'register', '--database', store, error='workspace_path_bound')['error']
    actor = Agent(None, 'human:device', project=ROOT, env=env)
    try:
        mcp = actor.call('workspace_register', {'database': str(store)}, error='workspace_path_bound')
        assert (mcp['code'], mcp['message']) == (refused['code'], refused['message'])
        listed = local('workspace', 'list')
        assert actor.call('workspace_list', {})['data'] == listed
        assert [(b['workspace'], b['store']) for b in listed] == [(first, {'status': 'identity_mismatch', 'found': second})]
        local('--project', checkout, 'status', error='workspace_identity_mismatch')
        started = mcp_start(checkout)
        assert started.returncode != 0 and 'workspace_identity_mismatch:' in started.stderr, started.stderr
        assert local('workspace', 'register', '--replace', '--database', store)['workspace'] == second
        listed = local('workspace', 'list')
        assert actor.call('workspace_list', {})['data'] == listed
        assert [(b['workspace'], b['store'], b['shared_with']) for b in listed] == [(second, {'status': 'ok'}, [])]
        store.unlink()
        assert actor.call('workspace_list', {})['data'] == local('workspace', 'list')
        assert local('workspace', 'list')[0]['store'] == {'status': 'missing'}
    finally:
        actor.close()
    (checkout / '.dpm/project.toml').write_text(f"version = 2\nworkspace = '{second}'\n")
    missing = local('--project', checkout, 'status', error='workspace_store_missing')['error']['message']
    started = mcp_start(checkout)
    assert started.returncode != 0 and f'workspace_store_missing: {missing}' in started.stderr, started.stderr


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-projects-') as temporary:
        directory = Path(temporary)
        preview(directory)
        normal(directory)
        git_evidence(directory)
        bindings(directory)
        stale_bindings(directory)
    print('PASS: project discovery/overrides, Git boundaries, read-only preview, CLI/MCP parity, durable operations, scoped Git evidence and stale device bindings')
