#!/usr/bin/env python3
"""Verify real CLI/MCP processes share structured queries and mutation behavior."""
import json
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / 'target/release/dpm'
MCP = ROOT / 'target/release/dpm-mcp'


class Agent:
    def __init__(self, database, actor, *, cwd=ROOT, project=None):
        selection = ['--db', str(database)] if database is not None else []
        if project is not None:
            selection = ['--project', str(project)]
        self.process = subprocess.Popen(
            [str(MCP), *selection, '--actor', actor],
            cwd=cwd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
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
        assert {'project_status', 'next_work', 'get_work', 'explain_work', 'claim_work', 'report_blocker', 'unblock_work', 'submit_work', 'verify_work', 'report_progress', 'add_artifact', 'attach_git_head', 'decide_gate'} == names
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
        worker.call('attach_git_head', {'key': 'TEST-A', 'base_revision': 3})
        run_cli(database, 'block', 'TEST-A', 'Waiting for fixture', '--actor', 'agent:parity')
        assert worker.call('next_work', {})['data'] == []
        worker.call('unblock_work', {'key': 'TEST-A', 'base_revision': 5})
        worker.call('submit_work', {'key': 'TEST-A', 'base_revision': 6, 'note': 'Acceptance evidence reviewed'})
        worker.call('verify_work', {'key': 'TEST-A', 'base_revision': 7}, error='invalid_command')
        run_cli(database, 'verify', 'TEST-A', '--actor', 'agent:parity', error='invalid_command')
        reviewer.call('verify_work', {'key': 'TEST-A', 'base_revision': 7})
        assert worker.call('next_work', {})['data'] == []
        reviewer.call('decide_gate', {'decision': 'TEST-GATE', 'outcome': 'Accept the verified input', 'base_revision': 8})
        next_work = worker.call('next_work', {})['data']
        assert next_work == run_cli(database, 'next')
        assert {candidate['work']['key'] for candidate in next_work} == {'TEST-B', 'TEST-D'}
        assert worker.call('project_status', {})['data']['revision'] == 9
    finally:
        worker.close()
        reviewer.close()


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='dpm-agent-') as directory:
        smoke(Path(directory) / 'plan.sqlite')
    print('PASS: CLI/MCP query parity, revision conflicts, evidence, blockers, gates and independent verification')
