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
        # A repeated field exists only in text; validate reads the file as import does and refuses it.
        repeated = directory / 'repeated-field.json'
        repeated.write_text(FIXTURE.read_text().replace('{', '{"revision": 999, ', 1))
        refused = run_cli(database, 'validate', str(repeated), error='invalid_request')['error']
        assert 'duplicate field' in refused['message'], refused
        assert run_cli(directory / 'repeated.sqlite', 'import', str(repeated), error='invalid_request')['error'] == refused
        # The schema describes the format, not the workspace: neither adapter reports a revision.
        schema = run_cli_envelope(database, 'plan', 'schema')
        assert agent.call('plan_schema', {}) == schema and schema['revision'] is None, schema['revision']
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


class Twins:
    """Two stores imported from one plan, advanced in lockstep: the CLI drives one, the agent tools
    the other, and every mutation must record the same Operation in the same envelope."""

    # Fields that differ between any two recordings of one command; each store starts its own
    # lineage when it is imported.
    RECORDING = ('id', 'timestamp', 'lineage_id')

    def __init__(self, directory, name, plan):
        fixture = directory / f'{name}.json'
        fixture.write_text(json.dumps(plan))
        self.directory, self.name = directory, name
        self.cli_db, self.mcp_db = directory / f'{name}-cli.sqlite', directory / f'{name}-mcp.sqlite'
        for database in (self.cli_db, self.mcp_db):
            run_cli(database, 'import', str(fixture))
        self.agents, self.revision, self.covered, self.aliases = {}, plan['revision'], set(), {}

    def agent(self, actor):
        if actor not in self.agents:
            self.agents[actor] = Agent(self.mcp_db, actor)
        return self.agents[actor]

    def mutate(self, actor, tool, arguments, *command):
        """Run one mutation through both adapters and compare everything but its recording."""
        local = run_cli_envelope(self.cli_db, '--base-revision', str(self.revision), *command, '--actor', actor)
        remote = self.agent(actor).call(tool, {**arguments, 'base_revision': self.revision})
        remote = self.alias(local, remote)
        normalized = [self.normalize(tool, value) for value in (local, remote)]
        assert normalized[0] == normalized[1], (self.name, tool, local, remote)
        assert local['revision'] == local['data']['resulting_revision'] == self.revision + 1, local
        self.revision += 1
        self.covered.add(tool)
        return local['data']

    # Identities each store mints for an entity a command creates, by command and field.
    MINTED = {'LinkExternal': 'reference'}

    def alias(self, local, remote):
        """Read the agent store's minted identities as the CLI store's, here and in later commands."""
        for command, body in local['data']['command'].items():
            field = self.MINTED.get(command)
            if field and command in remote['data']['command']:
                self.aliases[remote['data']['command'][command][field]] = body[field]
        text = json.dumps(remote)
        for minted, shared in self.aliases.items():
            text = text.replace(minted, shared)
        return json.loads(text)

    def normalize(self, tool, envelope):
        value = json.loads(json.dumps(envelope))
        for field in self.RECORDING:
            assert value['data'].pop(field), (tool, field, envelope)
        assert value.pop('lineage_id') == envelope['data']['lineage_id'], envelope
        if tool == 'attach_git_head':
            # Git evidence is captured at call time under a fresh identity.
            artifact = value['data']['command']['AttachArtifact']['artifact']
            assert artifact.pop('id') and artifact.pop('created_at'), artifact
        return value

    def apply(self, actor, reason, change):
        """Apply the same reviewed edit to each store's own export."""
        candidate = run_cli(self.cli_db, 'export')
        change(candidate)
        path = self.directory / f'{self.name}-candidate.json'
        path.write_text(json.dumps(candidate))
        remote = self.agent(actor).call('export_plan', {})['data']
        change(remote)
        return self.mutate(actor, 'apply_change', {'plan': remote, 'reason': reason},
                           'plan', 'apply', str(path), '--reason', reason)

    def finish(self):
        """Both stores end at the revision the compared operations produced."""
        assert run_cli(self.cli_db, 'export')['revision'] == run_cli(self.mcp_db, 'export')['revision'] == self.revision
        return self.covered

    def close(self):
        for agent in self.agents.values():
            agent.close()


def soften(plan):
    keys = {w['key']: w['id'] for w in plan['work_items'].values()}
    [edge] = [d for d in plan['dependencies'] if (d['predecessor'], d['successor']) == (keys['TEST-A'], keys['TEST-B'])]
    edge['policy'], edge['rationale'] = 'Soft', 'B may start from a prototype of A'


def lifecycle_parity(directory, reviewer, worker, heir):
    plan = json.loads(FIXTURE.read_text())
    next(w for w in plan['work_items'].values() if w['key'] == 'TEST-A')['execution']['status'] = 'Proposed'
    twins = Twins(directory, 'lifecycle', plan)
    try:
        twins.apply(reviewer, 'Allow prototype overlap', soften)
        edge = next(d['id'] for d in run_cli(twins.cli_db, 'export')['dependencies'] if d['policy'] == 'Soft')
        twins.mutate(reviewer, 'waive_dependency', {'dependency': edge, 'reason': 'Prototype suffices'},
                     'waive-dependency', edge, '--reason', 'Prototype suffices')
        twins.mutate(reviewer, 'restore_dependency', {'dependency': edge, 'reason': 'Prototype rejected'},
                     'restore-dependency', edge, '--reason', 'Prototype rejected')
        task = {'key': 'TEST-A'}
        twins.mutate(reviewer, 'ratify_contract', task, 'ratify', 'TEST-A')
        twins.mutate(worker, 'claim_work', task, 'claim', 'TEST-A')
        twins.mutate(worker, 'release_work', {**task, 'reason': 'Wrong task'}, 'release', 'TEST-A', '--reason', 'Wrong task')
        twins.mutate(worker, 'claim_work', task, 'claim', 'TEST-A')
        twins.mutate(reviewer, 'handoff_work', {**task, 'to': heir, 'reason': 'Rebalance'},
                     'handoff', 'TEST-A', '--to', heir, '--reason', 'Rebalance')
        twins.mutate(heir, 'start_work', task, 'start', 'TEST-A')
        twins.mutate(heir, 'report_progress', {**task, 'percent': 40, 'note': 'Half way'},
                     'progress', 'TEST-A', '40', '--note', 'Half way')
        twins.mutate(heir, 'report_blocker', {**task, 'blocker': 'Fixture missing'}, 'block', 'TEST-A', 'Fixture missing')
        twins.mutate(heir, 'unblock_work', task, 'unblock', 'TEST-A')
        kind, name = heir.split(':')
        artifact = {'id': '5f0f7c1e-8d7a-4b9e-9a51-3f3c2b1a0d11', 'kind': 'TestResult', 'uri': 'file:report.txt',
                    'label': 'Report', 'metadata': {}, 'created_by': {'kind': kind.capitalize(), 'name': name},
                    'created_at': '2026-09-01T00:00:00Z'}
        path = directory / 'parity-artifact.json'
        path.write_text(json.dumps(artifact))
        twins.mutate(heir, 'add_artifact', {**task, 'artifact': artifact}, 'artifact', 'TEST-A', str(path))
        twins.mutate(heir, 'attach_git_head', {**task, 'asset': 'TEST-REPO'}, 'attach-git-head', 'TEST-A', '--asset', 'TEST-REPO')
        identity = {'provider': 'Forgejo', 'instance': 'git.alpha.example', 'namespace': 'ops/dpm', 'kind': 'Issue', 'external_id': '42'}
        flags = ['--provider', 'Forgejo', '--instance', 'git.alpha.example', '--namespace', 'ops/dpm', '--kind', 'Issue', '--id', '42']
        twins.mutate(heir, 'link_external', {**task, 'identity': identity, 'observed': 'Open'},
                     'link-external', 'TEST-A', *flags, '--observed', 'open')
        twins.mutate(heir, 'unlink_external', {**task, 'identity': identity}, 'unlink-external', 'TEST-A', *flags)
        twins.mutate(heir, 'submit_work', {**task, 'note': 'Evidence attached'}, 'submit', 'TEST-A', '--note', 'Evidence attached')
        twins.mutate(reviewer, 'reject_work', {**task, 'reason': 'Report incomplete'}, 'reject', 'TEST-A', 'Report incomplete')
        twins.mutate(heir, 'submit_work', task, 'submit', 'TEST-A')
        twins.mutate(reviewer, 'verify_work', {**task, 'note': 'Checked'}, 'verify', 'TEST-A', '--note', 'Checked')
        twins.mutate(reviewer, 'decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Proceed'}, 'decide', 'TEST-GATE', 'Proceed')
        return twins.finish()
    finally:
        twins.close()


def basis_parity(directory, reviewer):
    """Re-basing needs a rejected attempt a started successor relied on provisionally."""
    from smoke_provisional import provisional_plan
    twins = Twins(directory, 'basis', provisional_plan())
    author, builder = 'agent:author', 'agent:builder'
    a, b = {'key': 'TEST-A'}, {'key': 'TEST-B'}
    try:
        for actor, key, command in [(author, a, 'claim'), (author, a, 'start'), (author, a, 'submit'),
                                    (builder, b, 'claim'), (builder, b, 'start')]:
            twins.mutate(actor, f'{command}_work', key, command, key['key'])
        twins.mutate(reviewer, 'reject_work', {**a, 'reason': 'Fails acceptance'}, 'reject', 'TEST-A', 'Fails acceptance')
        twins.mutate(author, 'submit_work', a, 'submit', 'TEST-A')
        twins.mutate(reviewer, 'verify_work', a, 'verify', 'TEST-A')
        edge = run_cli(twins.cli_db, 'explain', 'TEST-B')['basis']['relies_on'][0]['dependency']
        twins.mutate(reviewer, 'revalidate_basis', {**b, 'dependency': edge, 'attempt': 2, 'reason': 'B matches A2'},
                     'revalidate-basis', 'TEST-B', '--dependency', edge, '--attempt', '2', '--reason', 'B matches A2')
        return twins.finish()
    finally:
        twins.close()


def operation_smoke(directory):
    """Every mutation tool records, through either adapter, the same Operation."""
    reviewer = 'human:reviewer'
    covered = lifecycle_parity(directory, reviewer, 'agent:worker', 'agent:heir')
    covered |= basis_parity(directory, reviewer)
    probe = Agent(directory / 'lifecycle-mcp.sqlite', 'agent:probe')
    try:
        tools = probe.request('tools/list', {})['tools']
    finally:
        probe.close()
    # workspace_register binds a device-local path; it records no project operation.
    mutations = {t['name'] for t in tools if not t['annotations']['readOnlyHint']} - {'workspace_register'}
    assert covered == mutations, ('mutation tools without operation parity', mutations - covered, covered - mutations)


def table(text, header):
    """Rows of the Markdown table whose header row starts with `header`, as lists of cells."""
    lines = text.splitlines()
    start = next(i for i, line in enumerate(lines) if line.startswith(header))
    rows = []
    for line in lines[start + 2:]:
        if not line.startswith('|'):
            break
        rows.append([cell.strip() for cell in line.strip('|').split('|')])
    return rows


def subcommands(*path):
    """Subcommand names clap lists in `dpm [PATH] --help`, without its `help` pseudo-command."""
    shown = subprocess.run([str(CLI), *path, '--help'], capture_output=True, text=True, check=True, timeout=15).stdout
    listing = shown.split('Commands:\n', 1)[1].split('\n\n', 1)[0]
    return {line.split()[0] for line in listing.splitlines() if line.strip()} - {'help'}


def documentation_smoke(directory):
    """docs/mcp.md names every CLI command, every tool and every version number as the binaries do."""
    text = (ROOT / 'docs/mcp.md').read_text()
    rows = table(text, '| CLI | MCP tool |')
    documented = [' '.join(row[0].split()[:2]) if row[0].split()[0] in {'plan', 'workspace'} else row[0].split()[0] for row in rows]
    nested = {'plan', 'workspace'}
    commands = (subcommands() - nested) | {f'{group} {name}' for group in nested for name in subcommands(group)}
    assert len(documented) == len(set(documented)), 'a command is listed twice'
    assert set(documented) == commands, ('table differs from dpm --help', set(documented) ^ commands)
    for row in rows:
        assert row[1].startswith('CLI-only: ') or ' ' not in row[1], ('tool cell is a name or CLI-only: reason', row)
    tools = [row[1] for row in rows if not row[1].startswith('CLI-only')]
    database = directory / 'documentation.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    agent = Agent(database, 'agent:documentation')
    try:
        declared = {t['name']: t['_meta']['dpm/cli'] for t in agent.request('tools/list', {})['tools']}
        listed = set(declared)
        live = {
            'api_version': agent.call('project_status', {'probabilistic': False})['api_version'],
            'result_version': agent.call('next_work', {'probabilistic': False})['data']['result_version'],
            'format_version': agent.call('export_plan', {})['data']['format_version'],
            'schema_version': run_cli(database, 'verify-store')['schema_version'],
        }
    finally:
        agent.close()
    assert len(tools) == len(set(tools)) and set(tools) == listed, ('table differs from tools/list', set(tools) ^ listed)
    paired = {row[1]: command for row, command in zip(rows, documented) if not row[1].startswith('CLI-only')}
    assert paired == declared, ('table pairs differ from each tool\'s declared CLI command',
                                {t: (paired.get(t), declared.get(t)) for t in listed if paired.get(t) != declared.get(t)})
    assert run_cli_envelope(database, 'status', '--no-simulation')['api_version'] == live['api_version']
    versions = {row[0].strip('`'): int(row[1]) for row in table(text, '| Version | Value |')}
    assert versions == live, ('version table differs from live output', versions, live)
    assert f'"api_version": {live["api_version"]}' in text, 'the envelope example names another api_version'


def smoke(directory):
    validation_smoke(directory)
    bootstrap_smoke(directory)
    operation_smoke(directory)
    documentation_smoke(directory)
    print('PASS: CLI validate / validate_plan parity and refusals; agents cannot create or bootstrap a workspace')
    print('PASS: every mutation tool records the same Operation and envelope as its CLI command')
    print('PASS: docs/mcp.md maps every CLI command and tool and states the live version numbers')


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-adapters-') as directory:
        smoke(Path(directory))
