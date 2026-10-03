#!/usr/bin/env python3
"""The packaged DPM Observer app, run as shipped: a copy of the bundle is launched from the
filesystem root against disposable synthetic stores that this script writes through the real CLI,
and what it shows is read back from its own `--state-file`, the same snapshot the window renders.

Proven here, through the real SwiftUI process and its bundled helper:

* it opens only the workspace it was told to, labels a preview and a live store, and shows a missing
  or invalid project as an error without creating anything;
* the review queue, the runs and the blockers come from the shared queries, a decision that holds
  work back and work that no run has touched can be opened, and a completed run, silence and a quit
  change nothing about the work;
* an external commit appears within the two-second budget while the selection stays, a burst of
  activity stays bounded and leaves the main thread responsive, a lost helper is replaced, a
  repointed locator is reported and never followed, and a reported-only run that goes quiet is stale;
* quitting ends the helper and releases nothing.

The state file is watched with kqueue: a wait ends when the app rewrites it or exits, never by trying
again after a pause, and every wait has a deadline. Writes go only to disposable stores. The window
itself is judged by a person; see the operator scenario in docs/architecture.md. Skipped loudly, not
passed, off macOS.
"""
import json
import os
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from smoke_agent import ROOT, run_cli

FIXTURE = ROOT / 'tests/support/execution-plan.json'
BUDGET_MS = 2000
# The app's own main-thread monitor: the longest the main run loop went without a turn once connected.
STEADY_GAP_LIMIT_MS = 400
ACTIVITY_RUN = '0192f000-0000-7000-8000-0000000000a1'
STALE_RUN = '0192f000-0000-7000-8000-0000000000a2'
WORKER = 'agent:worker'
SERVICE = 'service:dpm-claude'
_apps = [0]


class App:
    """One launched copy of the packaged app and its state file, watched for changes."""

    def __init__(self, bundle, directory, arguments):
        _apps[0] += 1
        self.directory = directory / f'app-{_apps[0]}'
        self.directory.mkdir()
        self.state_file = self.directory / 'state.json'
        executable = bundle / 'Contents/MacOS/dpm-observer'
        self.process = subprocess.Popen([str(executable), *arguments, '--state-file', str(self.state_file)], cwd='/',
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        # The state is rewritten by rename inside this directory, so a write to it is the event; the
        # process exiting is the other.
        self.watched = os.open(self.directory, os.O_RDONLY)
        self.queue = select.kqueue()
        self.queue.control([
            select.kevent(self.watched, select.KQ_FILTER_VNODE, select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                          select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND | select.KQ_NOTE_RENAME | select.KQ_NOTE_LINK | select.KQ_NOTE_DELETE),
            select.kevent(self.process.pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_CLEAR, select.KQ_NOTE_EXIT),
        ], 0, 0)
        self.closed = False

    def state(self):
        try:
            return json.loads(self.state_file.read_text())
        except (OSError, ValueError):
            return None

    def wait(self, predicate, timeout=20, what='the app to show it'):
        """The first written state that satisfies `predicate`; otherwise a failure naming what was awaited."""
        deadline = time.monotonic() + timeout
        while True:
            last = self.state()
            if last is not None and predicate(last):
                return last
            if self.process.poll() is not None:
                raise AssertionError(f'the app exited ({self.process.returncode}) while waiting for {what}: {self.process.stdout.read()[-1500:]}')
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError(f'timed out waiting for {what}; last state: {json.dumps(last, sort_keys=True)[:1500]}')
            self.queue.control(None, 8, remaining)

    def quit(self):
        """Ask it to quit the way the menu does and report how it ended."""
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                raise AssertionError('the app did not quit within 15 seconds')
        return self.process.returncode

    def close(self):
        """End the app if it still runs, and release what watched it. Safe to call again."""
        if self.closed:
            return
        self.closed = True
        try:
            self.quit()
        finally:
            self.queue.close()
            os.close(self.watched)


def connected(state):
    return state['connection'] == 'connected' and state['freshness']['current']


def selected(state):
    return connected(state) and state['selection'] and state['detail'] and not state['detail']['loading']


def helpers_for(database):
    """Helper processes still holding this store: none should outlive their app."""
    result = subprocess.run(['/usr/bin/pgrep', '-f', f'dpm-native.*{database}'], capture_output=True, text=True)
    return [int(pid) for pid in result.stdout.split()]


def now_ms():
    return int(time.time() * 1000)


def store(directory, name):
    database = directory / f'{name}.sqlite'
    run_cli(database, 'import', str(FIXTURE))
    return database


def review_store(directory, name='review'):
    """TEST-A submitted for review after a managed run that asked a question, beside a reported-only
    run: what an operator opens the app to find. TEST-B and the others wait behind it and behind an
    open decision."""
    database = store(directory, name)
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'run', 'start', 'TEST-A', '--actor', SERVICE, '--run-id', ACTIVITY_RUN, '--observation', 'managed', '--executor', WORKER)
    for sequence, (kind, text) in enumerate([('progress', 'reading the contract'), ('tool_started', 'Read: contract.txt'),
                                              ('tool_result', 'Read ok'), ('input_requested', 'AskUserQuestion: proceed?')], 1):
        run_cli(database, 'run', 'record', ACTIVITY_RUN, kind, '--sequence', str(sequence), '--text', text, '--actor', SERVICE)
    run_cli(database, 'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', STALE_RUN)
    run_cli(database, 'run', 'record', STALE_RUN, 'heartbeat', '--sequence', '1', '--actor', WORKER)
    run_cli(database, 'submit', 'TEST-A', '--actor', WORKER, '--note', 'ready for the independent review')
    return database


def unrun_store(directory, name='unrun'):
    """Work that is started and that no run has touched."""
    database = store(directory, name)
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    return database


def review_and_verify(bundle, directory):
    database = review_store(directory)
    operations = len(run_cli(database, 'history')['entries'])
    app = App(bundle, directory, ['--database', str(database), '--select-key', 'TEST-A'])
    try:
        state = app.wait(selected, what='the review store, connected with TEST-A selected')
        assert state['source'] == 'live' and state['setup_error'] is None, state
        assert state['submitted_keys'] == ['TEST-A'] and state['counts']['runs'] == 2, state
        assert sorted(run['observation'] for run in state['runs']) == ['managed', 'reported_only'], state['runs']
        assert state['counts']['blockers'] > 0 and state['next_keys'] == [], 'nothing is ready, and the blockers say why'
        assert state['inspection']['views'] >= 4 and any(line.startswith('project feed') for line in state['inspection']['cursors']), state['inspection']
        # An independent verifier accepts the work from outside the app: the queue empties by itself.
        started = now_ms()
        run_cli(database, 'verify', 'TEST-A', '--actor', 'human:reviewer')
        after = app.wait(lambda s: s['submitted_keys'] == [] and connected(s), what='the review queue to empty after an external verify')
        assert after['selection']['key'] == 'TEST-A', 'the selection stayed on TEST-A through the refresh'
        latency = after['installed_at_ms'] - started
        assert latency < BUDGET_MS, f'an external verify took {latency} ms to show'
        assert len(run_cli(database, 'history')['entries']) == operations + 1, 'the app wrote nothing: only the reviewer\'s operation was added'
        code = app.quit()
        assert code == 0 and not helpers_for(database), (code, helpers_for(database))
        assert run_cli(database, 'show', 'TEST-A')['execution']['status'] == 'Verified', 'quitting verified or released nothing further'
        return f'review queue emptied in {latency} ms after an external verify'
    finally:
        app.close()


def reachable_work(bundle, directory):
    """A decision that holds work back, a task only that decision holds back, and started work that
    no run touched are each opened by the app with the contract text in full."""
    gated = store(directory, 'gated')
    for step in ('claim', 'start', 'submit'):
        run_cli(gated, step, 'TEST-A', '--actor', WORKER)
    run_cli(gated, 'verify', 'TEST-A', '--actor', 'human:reviewer')
    app = App(bundle, directory, ['--database', str(gated), '--select-decision', 'TEST-GATE', '--page', 'detail'])
    try:
        state = app.wait(selected, what='the open decision to be selected')
        assert state['selection']['kind'] == 'decision' and state['selection']['key'] == 'TEST-GATE', state['selection']
        assert {'Decision', 'Question'} <= set(state['detail']['sections']) and any(s.startswith('Work this decision gates') for s in state['detail']['sections']), state['detail']
        assert state['counts']['decisions'] >= 1 and state['counts']['blockers'] >= 1, state['counts']
    finally:
        app.close()
    app = App(bundle, directory, ['--database', str(gated), '--select-key', 'TEST-B', '--page', 'detail'])
    try:
        state = app.wait(lambda s: selected(s) and s['work_runs'], what='the decision-only blocked task')
        assert state['selection']['key'] == 'TEST-B' and 'Readiness' in state['detail']['sections'] and 'Acceptance criteria' in state['detail']['sections'], state['detail']
        assert state['work_runs']['runs'] == 0 and not state['work_runs']['truncated'], state['work_runs']
    finally:
        app.close()
    unrun = unrun_store(directory)
    app = App(bundle, directory, ['--database', str(unrun), '--select-key', 'TEST-A', '--page', 'detail'])
    try:
        state = app.wait(lambda s: selected(s) and s['work_runs'], what='started work with no run')
        assert state['counts']['runs'] == 0 and state['work_runs']['runs'] == 0 and not state['work_runs']['truncated'], state
        assert {'Summary', 'Acceptance criteria', 'Readiness'} <= set(state['detail']['sections']), state['detail']
    finally:
        app.close()
    return 'an open decision, the task only it holds back, and started work with no run are each found and read in full without a terminal'


def latency_and_burst(bundle, directory):
    database = store(directory, 'live')
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'run', 'start', 'TEST-A', '--actor', SERVICE, '--run-id', ACTIVITY_RUN, '--observation', 'managed', '--executor', WORKER)
    app = App(bundle, directory, ['--database', str(database), '--select-key', 'TEST-A'])
    try:
        state = app.wait(selected, what='the live store with TEST-A selected')
        revision = state['revision']
        worst = 0
        for percent in (20, 40, 60, 80, 100):
            revision += 1
            started = now_ms()
            run_cli(database, 'progress', 'TEST-A', str(percent), '--actor', WORKER)
            shown = app.wait(lambda s: s['revision'] == revision and s['selection'] and s['selection']['key'] == 'TEST-A' and s['detail'] and not s['detail']['loading'],
                             what=f'revision {revision} to appear with TEST-A still selected')
            worst = max(worst, shown['installed_at_ms'] - started)
        assert worst < BUDGET_MS, f'an external commit took {worst} ms to appear'
        # A burst of public activity from a run.
        run_cli(database, 'run', 'record', ACTIVITY_RUN, 'progress', '--sequence', '1', '--text', 'begin', '--actor', SERVICE)
        before = app.wait(connected, what='the run to be listed')
        records = 300
        started = now_ms()
        for sequence in range(2, records + 1):
            run_cli(database, 'run', 'record', ACTIVITY_RUN, 'tool_result', '--sequence', str(sequence), '--text', f'burst {sequence}', '--actor', SERVICE)
        after = app.wait(lambda s: s['runs'] and s['runs'][0]['recorded'] == records and connected(s), 60, 'the burst to be counted')
        gap = after['main_thread']['steady_max_gap_ms']
        assert gap is not None and gap < STEADY_GAP_LIMIT_MS, f'the main thread was held for {gap} ms'
        reads = after['counters']['project_reads'] - before['counters']['project_reads']
        assert reads == 0, f'{reads} project reads for run activity alone'
        assert after['counters']['run_reads'] - before['counters']['run_reads'] < 80, 'runs are read at most once per interval, not per record'
        assert after['state_write_failures'] == 0 and after['state_writes'] > 0, 'the state file was written without failure'
        code = app.quit()
        assert code == 0 and not helpers_for(database), (code, helpers_for(database))
        return (f'external commits appeared within {worst} ms (budget {BUDGET_MS} ms) with the selection kept; {records} activity records '
                f'in {now_ms() - started} ms left the main thread at most {gap:.0f} ms behind')
    finally:
        app.close()


def selected_run_window(bundle, directory):
    database = store(directory, 'window')
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'run', 'start', 'TEST-A', '--actor', SERVICE, '--run-id', ACTIVITY_RUN, '--observation', 'managed', '--executor', WORKER)
    run_cli(database, 'run', 'record', ACTIVITY_RUN, 'progress', '--sequence', '1', '--text', 'before the app', '--actor', SERVICE)
    app = App(bundle, directory, ['--database', str(database), '--select-run', ACTIVITY_RUN, '--page', 'live'])
    try:
        app.wait(lambda s: connected(s) and s['window'] and s['window']['entries'] == 1, what="the selected run's window")
        run_cli(database, 'run', 'record', ACTIVITY_RUN, 'tool_started', '--sequence', '2', '--text', 'Read: a.txt', '--actor', SERVICE)
        shown = app.wait(lambda s: s['window'] and s['window']['entries'] == 2, what='a new activity record to appear in the window')
        assert shown['page'] == 'live' and shown['window']['gap'] is None, shown
        # The helper is killed: the loss is shown, replaced, and the window continues.
        for pid in helpers_for(database):
            os.kill(pid, signal.SIGKILL)
        lost = app.wait(lambda s: s['connection'] == 'reconnecting', what='the lost helper to be shown')
        assert not lost['freshness']['current'] and lost['window'] and lost['counts']['inventory'] > 0, 'what was shown stays, marked not current'
        run_cli(database, 'run', 'record', ACTIVITY_RUN, 'tool_result', '--sequence', '3', '--text', 'written during the loss', '--actor', SERVICE)
        back = app.wait(lambda s: connected(s) and s['counters']['reconnects'] >= 1 and s['window'] and s['window']['entries'] == 3, 30, 'the helper to be replaced and the window caught up')
        assert back['selection']['kind'] == 'run' and back['counters']['resets'] == 0, back
        code = app.quit()
        assert code == 0 and not helpers_for(database), (code, helpers_for(database))
        return 'a killed helper was shown, replaced, and the selected run continued without a reset'
    finally:
        app.close()


def preview_and_refusals(bundle, directory):
    locator = (ROOT / '.dpm/project.toml').read_bytes()
    app = App(bundle, directory, ['--project', str(ROOT)])
    try:
        state = app.wait(connected, what="this repository's read-only preview")
        assert state['source'] == 'preview' and state['lineage'] is None and state['revision'] == 0, state
        assert state['next_keys'] == [] and state['counts']['blockers'] > 0, 'the open gates keep the prepared roadmap unstarted, and the app says what holds it'
        assert state['counts']['submitted'] == 0 and state['counts']['inventory'] > 50 and state['counts']['decisions'] > 0, state['counts']
        assert (ROOT / '.dpm/project.toml').read_bytes() == locator, 'the preview locator was not touched'
        assert app.quit() == 0
    finally:
        app.close()
    # A directory with no project is an error on screen, and nothing is created in it.
    empty = directory / 'not-a-project'
    empty.mkdir()
    missing = App(bundle, directory, ['--project', str(empty)])
    try:
        failed = missing.wait(lambda s: s['connection'] == 'failed', what='a visible error for a directory with no project')
        assert failed['connection_detail'] and list(empty.iterdir()) == [], (failed['connection_detail'], list(empty.iterdir()))
        gone = App(bundle, directory, ['--database', str(directory / 'absent.sqlite')])
        try:
            gone.wait(lambda s: s['connection'] == 'failed', what='a visible error for a missing store')
            assert not (directory / 'absent.sqlite').exists(), 'a missing store was created'
        finally:
            gone.close()
        assert missing.process.poll() is None, 'the app stays open to show the error'
    finally:
        missing.close()
    return 'the read-only preview opens labelled and untouched; a missing project or store is an error on screen and creates nothing'


def stale_and_source_change(bundle, directory):
    database = store(directory, 'stale')
    run_cli(database, 'claim', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'start', 'TEST-A', '--actor', WORKER)
    run_cli(database, 'run', 'start', 'TEST-A', '--actor', WORKER, '--run-id', STALE_RUN)
    run_cli(database, 'run', 'record', STALE_RUN, 'heartbeat', '--sequence', '1', '--actor', WORKER)
    later = time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(time.time() + 600))
    app = App(bundle, directory, ['--database', str(database), '--clock', later])
    try:
        state = app.wait(connected, what='the store at a clock ten minutes on')
        run = state['runs'][0]
        assert (run['observation'], run['status'], run['state']) == ('reported_only', 'stale', 'working'), run
        assert run_cli(database, 'show', 'TEST-A')['execution']['status'] == 'InProgress'
    finally:
        app.close()
    # A project whose locator is repointed while the app is open.
    workspace = run_cli(database, 'export')['workspace']['id']
    project = directory / 'repoint'
    (project / '.dpm').mkdir(parents=True)
    for name in ('a', 'b'):
        run_cli(project / '.dpm' / f'{name}.sqlite', 'import', str(FIXTURE))

    def locate(target):
        (project / '.dpm/project.toml').write_text(f"version = 3\nworkspace = '{workspace}'\ndatabase = '{target}'\n")

    locate('a.sqlite')
    app = App(bundle, directory, ['--project', str(project)])
    try:
        app.wait(connected, what='the project at its first source')
        locate('b.sqlite')
        changed = app.wait(lambda s: s['connection'] == 'source_changed', what='the repointed locator to be reported')
        assert changed['freshness']['project_stale'] and changed['counts']['inventory'] > 0, 'the previous view stays, marked stale'
        return 'a reported-only run ten minutes silent is stale while its report stays working; a repointed locator is reported, not followed'
    finally:
        app.close()


def main(bundle):
    if sys.platform != 'darwin':
        print(f'SKIPPED: the observer is a macOS app and this is {sys.platform}; nothing was launched and nothing is claimed.')
        return []
    results = []
    with tempfile.TemporaryDirectory() as scratch:
        directory = Path(scratch)
        packaged = directory / 'DPMObserver.app'
        shutil.copytree(bundle, packaged, symlinks=True)
        for scenario in (review_and_verify, reachable_work, latency_and_burst, selected_run_window, preview_and_refusals, stale_and_source_change):
            results.append(scenario(packaged, directory))
    return results


if __name__ == '__main__':
    import build_native
    if '--prepare' in sys.argv:
        # Disposable stores for a person to open the app on; nothing is launched here.
        target = Path(sys.argv[sys.argv.index('--prepare') + 1])
        target.mkdir(parents=True, exist_ok=True)
        if (target / 'review.sqlite').exists():
            raise SystemExit(f"{target / 'review.sqlite'} already exists; use a new directory")
        review, unrun = review_store(target), unrun_store(target)
        bundle = build_native.target_directory() / 'native/DPMObserver.app'
        print(f'prepared {review} (work for review, a decision-gated task, a managed and a reported-only run)')
        print(f'prepared {unrun} (started work that no run has touched)')
        print('build:  python3 scripts/build_native.py')
        print(f'open:   open {bundle} --args --database {review}')
        print(f'then:   dpm --database {review} verify TEST-A --actor human:reviewer')
        raise SystemExit(0)
    built = build_native.build(rust='--no-rust' not in sys.argv)
    for line in main(built['observer']):
        print(f'PASS: {line}')
