#!/usr/bin/env python3
"""Bound default release-query latency and compare CLI/MCP on representative large graphs."""
import argparse
import copy
import json
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

from smoke_agent import Agent, CLI, ROOT, pinned_clock, run_cli


def generated(count, shape):
    plan = json.loads((ROOT / 'tests/support/execution-plan.json').read_text())
    task = next(w for w in plan['work_items'].values() if w['key'] == 'TEST-A')
    plan.update(work_items={}, dependencies=[], decisions={}, risks={})
    identity = lambda i: str(uuid.UUID(int=(77 << 96) + i + 1))
    for i in range(count):
        work = copy.deepcopy(task)
        work.update(id=identity(i), key=f'PERF-{i + 1}', order=[i + 1])
        work['schedule']['estimate'] = dict(optimistic_hours=1., likely_hours=2., pessimistic_hours=3.)
        plan['work_items'][work['id']] = work
        if i and shape != 'flat':
            parent = (i - 1) // 3 if shape == 'branch' else i - 1
            plan['dependencies'].append(dict(id=str(uuid.UUID(int=(78 << 96) + i)),
                predecessor=identity(parent), successor=work['id'], kind='FinishStart', lag_hours=0.))
    if shape == 'calendar':
        # A chain of human work on the Standard calendar: every pass does calendar arithmetic.
        plan['calendars'] = {'time_zone': 'Europe/Berlin'}
    return plan


def timed_cli(binary, database, args):
    start = time.perf_counter()
    result = subprocess.run([str(binary), '--json', '--database', str(database), *args],
                            capture_output=True, check=True, timeout=60, cwd=ROOT)
    return result.stdout, time.perf_counter() - start


def smoke(directory, count=5000, budget=5., baseline=None):
    results = []
    for shape in ('flat', 'chain', 'branch', 'calendar'):
        source, database = directory / f'{shape}.json', directory / f'{shape}.sqlite'
        source.write_text(json.dumps(generated(count, shape)))
        run_cli(database, 'import', str(source))
        worker = Agent(database, 'agent:performance')
        # Calendar forecasts depend on the time of day, so both adapters read one pinned clock.
        clock = pinned_clock()
        observer = Agent(database, 'agent:performance', clock=clock)
        try:
            queries = [(('status',), 'project_status', {}), (('next',), 'next_work', {}),
                       (('explain', 'PERF-1'), 'explain_work', {'key': 'PERF-1'})]
            for command, tool, arguments in queries:
                output, elapsed = timed_cli(CLI, database, ('--clock', clock, *command))
                assert elapsed < budget, (shape, command, elapsed, budget)
                # A baseline binary predating calendars cannot read the calendar plan.
                if baseline and shape != 'calendar':
                    previous, _ = timed_cli(baseline, database, ('--clock', clock, *command))
                    assert previous == output, (shape, command, 'query output changed')
                remote = observer.call(tool, arguments)
                assert remote == json.loads(output), (shape, tool, 'adapter divergence')
                results.append(dict(shape=shape, tasks=count, query=command[0], seconds=elapsed))
            started = time.perf_counter()
            worker.call('claim_work', {'key': 'PERF-1', 'base_revision': 0})
            results.append(dict(shape=shape, tasks=count, query='warm_claim', seconds=time.perf_counter() - started))
            # An external writer must invalidate the warm agent snapshot immediately.
            run_cli(database, 'start', 'PERF-1', '--actor', 'agent:performance')
            assert worker.call('get_work', {'key': 'PERF-1'})['data'] == run_cli(database, 'show', 'PERF-1')
        finally:
            worker.close()
            observer.close()
    return results


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tasks', type=int, default=5000)
    parser.add_argument('--budget', type=float, default=5., help='generous CI allowance in seconds per default query')
    parser.add_argument('--baseline', type=Path, help='optional release binary for byte comparison')
    parser.add_argument('--report', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='dpm-performance-') as temporary:
        results = smoke(Path(temporary), args.tasks, args.budget, args.baseline)
    if args.report:
        args.report.write_text(json.dumps(results, indent=2) + '\n')
    print(f'PASS: {args.tasks}-task flat/chain/branch queries below {args.budget:g}s; CLI/MCP parity, warm claims and external refresh')
    for result in results:
        print(f"  {result['shape']} {result['query']}: {result['seconds']:.3f}s")
