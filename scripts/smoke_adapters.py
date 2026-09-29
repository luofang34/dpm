#!/usr/bin/env python3
"""The CLI and the agent tools form one contract: plan validation, bootstrap policy and results."""
import json
import subprocess
import tempfile
from pathlib import Path

from smoke_agent import CLI, MCP, ROOT, Agent, run_cli, run_cli_envelope

FIXTURE = ROOT / 'tests/support/execution-plan.json'


def validation_smoke(directory):
    """`validate FILE` and `validate_plan` return the same envelope and the same refusals."""
    database = directory / 'validation.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    agent = Agent(database, 'agent:validator')
    try:
        plan = json.loads(FIXTURE.read_text())
        local = run_cli_envelope(database, 'validate', str(FIXTURE))
        assert agent.call('validate_plan', {'plan': plan}) == local, local
        assert local['revision'] is None and local['data']['valid'] is True, local
        cases = {
            'future-format': ('invalid_request', lambda p: p.update(format_version=4)),
            'dangling-edge': ('invalid_plan', lambda p: p['dependencies'][0].update(predecessor='00000000-0000-4000-8000-000000000000')),
        }
        for name, (code, damage) in cases.items():
            broken = json.loads(FIXTURE.read_text())
            damage(broken)
            candidate = directory / f'{name}.json'
            candidate.write_text(json.dumps(broken))
            remote = agent.call('validate_plan', {'plan': broken}, error=code)
            assert remote == run_cli(database, 'validate', str(candidate), error=code)['error'], name
        assert run_cli(database, 'history')['entries'] == []
    finally:
        agent.close()


def bootstrap_smoke(directory):
    """An agent process cannot create a workspace: it opens only existing stores and projects."""
    empty = directory / 'empty-directory'
    empty.mkdir()
    for selection in (['--db', str(empty / 'absent.sqlite')], ['--project', str(empty)], []):
        started = subprocess.run([str(MCP), *selection, '--actor', 'agent:bootstrap'], cwd=empty,
                                 stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30)
        assert started.returncode != 0, (selection, started)
        assert list(empty.iterdir()) == [], (selection, list(empty.iterdir()))
    database = directory / 'bootstrap.sqlite'
    run_cli(database, 'init', 'Bootstrap probe')
    agent = Agent(database, 'agent:bootstrap')
    try:
        tools = agent.request('tools/list', {})['tools']
        writers = [t for t in tools if not t['annotations']['readOnlyHint'] and t['name'] != 'workspace_register']
        assert all('base_revision' in t['inputSchema']['required'] for t in writers), writers
        template = agent.call('plan_template', {})['data']
        refused = agent.call('apply_change', {'plan': template, 'reason': 'self-bootstrap', 'base_revision': 0},
                             error='invalid_command')
        assert refused['message'] == run_cli(database, 'plan', 'apply', str(write(directory, template)), '--reason',
                                             'self-bootstrap', '--actor', 'agent:bootstrap', error='invalid_command')['error']['message']
        assert run_cli(database, 'export')['revision'] == 0
    finally:
        agent.close()


def write(directory, plan):
    path = directory / 'bootstrap-candidate.json'
    path.write_text(json.dumps(plan))
    return path


def smoke(directory):
    validation_smoke(directory)
    bootstrap_smoke(directory)
    print('PASS: CLI validate / validate_plan parity and refusals; agents cannot create or bootstrap a workspace')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-adapters-') as directory:
        smoke(Path(directory))
