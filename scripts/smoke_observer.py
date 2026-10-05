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
import copy
import ctypes
import hashlib
import json
import os
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

import measure_gantt
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


def gantt_store(directory, name, calendars=False):
    """The nested fixture of the Gantt (a package, a nested task, a milestone, the four relation kinds)
    imported through the real CLI into a disposable store."""
    plan = measure_gantt.write_plan(directory / f'{name}.json', measure_gantt.fx_gantt(calendars))
    database = directory / f'{name}.sqlite'
    run_cli(database, 'import', str(plan))
    return database


def gantt_loaded(state):
    return connected(state) and state['gantt']['loaded'] and state['gantt']['rows'] > 0


def gantt_page(bundle, directory):
    """The packaged app opened on `--page gantt`: its rows are the shared schedule's, the schedule is
    displayed only while the Gantt is shown, a lost helper is marked with the rows kept, and nothing is written."""
    database = gantt_store(directory, 'gantt')
    work = run_cli(database, 'schedule')['work']
    operations = len(run_cli(database, 'history')['entries'])
    other = App(bundle, directory, ['--database', str(database), '--page', 'now', '--select-key', 'TEST-A'])
    try:
        elsewhere = other.wait(lambda s: selected(s) and s['work_runs'] and not s['gantt']['loaded'], what='the store on the Now page, with TEST-A selected')
        assert elsewhere['gantt']['total_rows'] == 0, 'a page that is not the Gantt reads no schedule'
        views_without = elsewhere['inspection']['views']
        assert other.quit() == 0
    finally:
        other.close()
    app = App(bundle, directory, ['--database', str(database), '--page', 'gantt', '--select-key', 'TEST-A'])
    try:
        state = app.wait(lambda s: gantt_loaded(s) and selected(s) and s['work_runs'], what='the Gantt rows read, with TEST-A selected')
        gantt = state['gantt']
        assert state['page'] == 'gantt' and state['setup_error'] is None, state
        assert gantt['total_rows'] == len(work) and gantt['rows'] == len(work), f"rows {gantt['rows']} of {gantt['total_rows']}, the schedule query has {len(work)}"
        assert gantt['keys'] == [w['key'] for w in hierarchy_order(work)], gantt['keys']
        assert gantt['selected_key'] == 'TEST-A' and gantt['relations'] >= 10, gantt
        assert state['inspection']['views'] == views_without + 1, f"the schedule is one more displayed view on the Gantt ({views_without} elsewhere, {state['inspection']['views']} here)"
        assert 'focus_reported' in gantt and 'focus_target' in gantt and gantt['cursor_key'], gantt
        # The helper is killed: the loss is marked and the last view, the selection and the rows stay.
        for pid in helpers_for(database):
            os.kill(pid, signal.SIGKILL)
        lost = app.wait(lambda s: s['connection'] == 'reconnecting', what='the lost helper to be shown on the Gantt')
        assert not lost['freshness']['current'] and lost['gantt']['loaded'] and lost['gantt']['rows'] == len(work), 'the rows stay, marked not current'
        assert lost['gantt']['selected_key'] == 'TEST-A', 'the selection stays through the loss'
        back = app.wait(lambda s: gantt_loaded(s) and s['counters']['reconnects'] >= 1, 30, 'the helper to be replaced and the rows read again')
        assert back['gantt']['rows'] == len(work) and back['gantt']['selected_key'] == 'TEST-A', back['gantt']
        code = app.quit()
        assert code == 0 and not helpers_for(database), (code, helpers_for(database))
        assert len(run_cli(database, 'history')['entries']) == operations, 'the Gantt wrote nothing'
        return (f"the Gantt page showed the schedule's {len(work)} rows, listed the schedule under the observation basis only while shown "
                f"({views_without} views elsewhere, {state['inspection']['views']} here), kept its rows, selection and cursor through a killed helper, and wrote nothing "
                f"(focus as the view reports it: {back['gantt']['focus_reported']})")
    finally:
        app.close()


def hierarchy_order(work):
    """The hierarchy order of the rows, from the query's own `parent` fields: a package, then its children."""
    ids = {w['id'] for w in work}
    children = {}
    roots = []
    for item in work:
        if item['parent'] in ids:
            children.setdefault(item['parent'], []).append(item)
        else:
            roots.append(item)
    order = []

    def walk(item):
        order.append(item)
        for child in children.get(item['id'], []):
            walk(child)
    for root in roots:
        walk(root)
    return order


# Virtual key codes of the keys the Gantt reads (ANSI layout) and the shift flag.
KEY_CODES = {'down': 125, 'up': 126, 'left': 123, 'right': 124, 'space': 49, 'return': 36, 'escape': 53, 'tab': 48, 'page_down': 121, 'end': 119, 'home': 115,
             'slash': 44, 'c': 8, 'a': 0, 'delete': 51}
SHIFT_FLAG = 0x20000
COMMAND_FLAG = 0x100000


def accessibility_trusted():
    """Whether this process may post key events to another process: the Accessibility permission of the
    process that sends them. Without it nothing is posted and the case is skipped loudly."""
    try:
        library = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
        library.AXIsProcessTrusted.restype = ctypes.c_bool
        return bool(library.AXIsProcessTrusted())
    except (OSError, AttributeError):
        return False


def post_key(pid, code, flags=0):
    """One key press delivered to one process with CGEventPostToPid."""
    quartz = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
    core = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
    quartz.CGEventCreateKeyboardEvent.restype = ctypes.c_void_p
    quartz.CGEventCreateKeyboardEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint16, ctypes.c_bool]
    quartz.CGEventSetFlags.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
    quartz.CGEventPostToPid.argtypes = [ctypes.c_int, ctypes.c_void_p]
    core.CFRelease.argtypes = [ctypes.c_void_p]
    for down in (True, False):
        event = quartz.CGEventCreateKeyboardEvent(None, code, down)
        if flags:
            quartz.CGEventSetFlags(event, flags)
        quartz.CGEventPostToPid(pid, event)
        core.CFRelease(event)
        time.sleep(0.02)


def gantt_real_keys(bundle, directory):
    """Real key events (CGEventPostToPid) into the packaged app, read back from its state file. They need the
    Accessibility permission of this process: without it the case is SKIPPED LOUDLY and never passes."""
    if not accessibility_trusted():
        print('SKIPPED LOUDLY (not a pass): real key events into the packaged Gantt were NOT run. This process has no Accessibility permission '
              '(AXIsProcessTrusted is false), so CGEventPostToPid would be ignored. The shared key handler is exercised by the Swift suite with '
              'NSEvent values it builds (gantt-keys), not by this case; the keyboard walk with the accessibility inspector is for a person.')
        return None
    database = gantt_store(directory, 'gantt-keys')
    work = run_cli(database, 'schedule')['work']
    order = [w['key'] for w in hierarchy_order(work)]
    package = order.index('TEST-PKG')
    app = App(bundle, directory, ['--database', str(database), '--page', 'gantt'])
    try:
        app.wait(gantt_loaded, what='the Gantt rows')
        subprocess.run(['/usr/bin/osascript', '-e', f'tell application "System Events" to set frontmost of (first process whose unix id is {app.process.pid}) to true'],
                       capture_output=True, timeout=15)
        post_key(app.process.pid, KEY_CODES['down'])
        try:
            app.wait(lambda s: s['gantt']['cursor_key'] != order[0], timeout=5, what='the first real down arrow to move the cursor')
        except AssertionError:
            print('SKIPPED LOUDLY (not a pass): Accessibility is granted and a real down arrow was posted to the app, but the cursor did not move, so the '
                  'app may not have been the key window. Nothing is claimed about the keyboard path of the packaged app.')
            return None
        for _ in range(package - 1):
            post_key(app.process.pid, KEY_CODES['down'])
        at_package = app.wait(lambda s: s['gantt']['cursor_key'] == 'TEST-PKG', what='the cursor on the package')
        assert at_package['gantt']['focus_target'] == 'gantt.row.TEST-PKG', at_package['gantt']
        post_key(app.process.pid, KEY_CODES['right'])
        app.wait(lambda s: s['gantt']['cursor_key'] == order[package + 1], what='right arrow to go in to the nested task')
        post_key(app.process.pid, KEY_CODES['space'])
        app.wait(lambda s: s['gantt']['selected_key'] == order[package + 1], what='space to select the nested task')
        post_key(app.process.pid, KEY_CODES['return'])
        opened = app.wait(lambda s: s['page'] == 'detail', what='return to open Detail')
        assert opened['gantt']['detail_return'] == f'gantt.row.{order[package + 1]}', opened['gantt']
        post_key(app.process.pid, KEY_CODES['escape'])
        back = app.wait(lambda s: s['page'] == 'gantt' and s['gantt']['focus_target'] == f'gantt.row.{order[package + 1]}', what='escape to return to the Gantt row')
        assert back['gantt']['selected_key'] == order[package + 1]
        return f"real key events (CGEventPostToPid) moved the cursor, selected the nested task, opened Detail and returned focus to its row (focus as the view reports it: {back['gantt']['focus_reported']})"
    finally:
        app.close()


def png_facts(path):
    data = Path(path).read_bytes()
    assert data[:8] == b'\x89PNG\r\n\x1a\n', f'{path} is not a PNG'
    width, height = int.from_bytes(data[16:20], 'big'), int.from_bytes(data[20:24], 'big')
    return dict(bytes=len(data), width=width, height=height, sha256=hashlib.sha256(data).hexdigest())


def gantt_rendered(bundle, directory):
    """The app renders its own Gantt window contents to PNG (`--render-to`), for the stated scenarios. Only when
    DPM_RENDER_OUT names a directory; nothing is written otherwise. Every file is app-rendered, scripted and
    synthetic, and is not an operator observation."""
    target = os.environ.get('DPM_RENDER_OUT')
    if not target:
        print('SKIPPED (not asked): DPM_RENDER_OUT is not set, so the app was not asked to render and no PNG was written.')
        return None
    out = Path(target)
    out.mkdir(parents=True, exist_ok=True)
    produced = {}

    def wait_files(app, names, what):
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if all((out / name).is_file() and (out / name).stat().st_size > 0 for name in names):
                time.sleep(0.3)
                return
            if app.process.poll() is not None:
                raise AssertionError(f'the app exited while rendering {what}')
            time.sleep(0.2)
        raise AssertionError(f'{what}: {[n for n in names if not (out / n).is_file()]} were not rendered within 120 s')

    def render(name, arguments, names, prepare=None, store=None):
        app = App(bundle, directory, [*arguments, '--page', 'gantt', '--render-to', str(out), '--render-prefix', name, '--render-when', 'loaded'])
        try:
            app.wait(gantt_loaded, 60, f'{name} to load')
            if prepare:
                prepare(app)
            wait_files(app, names, name)
            for file in names:
                produced[file] = png_facts(out / file)
            assert app.quit() == 0
        finally:
            app.close()

    fixture = gantt_store(directory, 'render-fx')
    render('gantt-fx', ['--database', str(fixture)], ['gantt-fx-expanded.png', 'gantt-fx-collapsed.png', 'gantt-fx-selected-nested-detail.png'])
    # Disconnected: the helper is killed and the last reading is marked.
    app = App(bundle, directory, ['--database', str(fixture), '--page', 'gantt', '--render-to', str(out), '--render-prefix', 'gantt-fx', '--render-when', 'disconnected'])
    try:
        app.wait(gantt_loaded, 60, 'the fixture to load for the disconnected scenario')
        for pid in helpers_for(fixture):
            os.kill(pid, signal.SIGKILL)
        wait_files(app, ['gantt-fx-disconnected.png'], 'the disconnected scenario')
        produced['gantt-fx-disconnected.png'] = png_facts(out / 'gantt-fx-disconnected.png')
    finally:
        app.close()
    # Stale: a repointed locator is reported, never followed, and the previous source's rows stay, marked.
    workspace = run_cli(fixture, 'export')['workspace']['id']
    project = directory / 'render-repoint'
    (project / '.dpm').mkdir(parents=True)
    plan = measure_gantt.write_plan(directory / 'render-repoint.json', measure_gantt.fx_gantt())
    for name in ('a', 'b'):
        run_cli(project / '.dpm' / f'{name}.sqlite', 'import', str(plan))

    def locate(database):
        (project / '.dpm/project.toml').write_text(f"version = 3\nworkspace = '{workspace}'\ndatabase = '{database}'\n")

    locate('a.sqlite')
    app = App(bundle, directory, ['--project', str(project), '--page', 'gantt', '--render-to', str(out), '--render-prefix', 'gantt-fx', '--render-when', 'stale'])
    try:
        app.wait(gantt_loaded, 60, 'the project to load for the stale scenario')
        locate('b.sqlite')
        wait_files(app, ['gantt-fx-stale.png'], 'the stale scenario')
        produced['gantt-fx-stale.png'] = png_facts(out / 'gantt-fx-stale.png')
    finally:
        app.close()
    # A long title, and a 1000-task plan.
    longer = directory / 'render-long.sqlite'
    run_cli(longer, 'import', str(measure_gantt.write_plan(directory / 'render-long.json', measure_gantt.dataset(100, 'branch', long_titles=True))))
    render('gantt-long-title', ['--database', str(longer)], ['gantt-long-title-expanded.png', 'gantt-long-title-collapsed.png', 'gantt-long-title-selected-nested-detail.png'])
    big = directory / 'render-1000.sqlite'
    run_cli(big, 'import', str(measure_gantt.write_plan(directory / 'render-1000.json', measure_gantt.dataset(1000, 'branch'))))
    render('gantt-1000-tasks', ['--database', str(big)], ['gantt-1000-tasks-expanded.png', 'gantt-1000-tasks-collapsed.png', 'gantt-1000-tasks-selected-nested-detail.png'])
    assert all(facts['bytes'] > 20000 and facts['width'] > 1000 for facts in produced.values()), produced
    assert len({facts['sha256'] for facts in produced.values()}) == len(produced), 'two scenarios produced identical images'
    return f'{len(produced)} PNGs, app-rendered, scripted, synthetic (not an operator observation), written under {out}: ' + ', '.join(sorted(produced))


def no_tick_message(events):
    """The failure text when no display-link tick was logged: still a failure, with the display facts the app
    itself observed (the link's binding, the screens, the window's visibility and occlusion, whether the app is
    active, whether the link is paused, the first tick or its absence). Nothing here is inferred."""
    states = [{**e.get('detail', {}), 't_ns': e.get('t_ns')} for e in events.get('display_state', [])]
    link = [e.get('detail', {}) for e in events.get('display_link', []) + events.get('no_display', [])]
    return 'no display-link tick was logged; NO_TICKS; display facts observed by the app: ' + json.dumps(dict(display_link=link, display_state=states), sort_keys=True)


def gantt_instruments(bundle, directory):
    """The measurement instrument end to end through the packaged app: the log carries the process entry, the
    display-link ticks, a first draw stamped by the Gantt's own draw pass, selections drawn in Detail and a
    closing line with nothing dropped; and with the Gantt's draws withheld (the negative control) the model
    advances and the ticks continue but no first draw ever exists."""
    database = gantt_store(directory, 'gantt-measure')
    rows = len(run_cli(database, 'schedule')['work'])
    log = directory / 'instrument.jsonl'
    run = measure_gantt.Launched(bundle, directory, 'instrument', ['--database', str(database), '--page', 'detail', '--measure-script', 'selections', '--measure-keys', 'TEST-A,TEST-B,TEST-C'])
    try:
        assert run.wait_event('driver_done', 60) is not None, 'the selection driver did not finish'
        run.quit()
        events = run.by_name
        assert events.get('process_entry') and events.get('log_opened'), 'no header or process entry'
        assert events.get('tick'), no_tick_message(events)
        drawn = {e['gen']: e for e in events.get('detail_drawn', [])}
        calls = events.get('select_call', [])
        assert len(calls) == 3 and all(c['gen'] in drawn and drawn[c['gen']]['t_ns'] > c['t_ns'] for c in calls), 'each selection must be drawn after it was called'
        assert events['log_closed'][0]['dropped'] == 0, events['log_closed']
        latencies = [measure_gantt.ms(c['t_ns'], drawn[c['gen']]['t_ns']) for c in calls]
    finally:
        run.close()
    # The Gantt's first draw is stamped by its own draw pass with the full plan in the model.
    gantt = measure_gantt.Launched(bundle, directory, 'instrument-gantt', ['--database', str(database), '--page', 'gantt'])
    try:
        first = gantt.wait_event('first_draw', 30)
        assert first is not None and first['detail']['model_rows'] == rows and first['detail']['viewport_rows_drawn'] >= 1, first
        gantt.quit()
    finally:
        gantt.close()
    # The negative control: gantt frozen, the model advances, ticks continue, no first draw.
    frozen = measure_gantt.Launched(bundle, directory, 'instrument-frozen', ['--database', str(database), '--page', 'gantt'], freeze=('gantt',))
    try:
        frozen.wait_state(lambda s: connected(s) and s['gantt']['loaded'], 30, 'the model to read the schedule while the Gantt draw is withheld')
        time.sleep(3)
        frozen.quit()
        assert not frozen.by_name.get('first_draw'), 'a withheld draw must never complete a first draw'
        assert frozen.by_name.get('tick'), no_tick_message(frozen.by_name)
        assert frozen.by_name.get('state_set'), 'the model must have carried on'
    finally:
        frozen.close()
    return (f'the measurement log recorded the process entry, display-link ticks, 3 selections drawn in Detail ({", ".join(f"{v:.0f}" for v in latencies)} ms, in-app, lower bounds), '
            f'a stamped first draw of {rows} rows, and nothing dropped; with the Gantt draw withheld no first draw existed while ticks and the model continued')


def gantt_scroll_guards(bundle, directory):
    """The scroll frame stamp on the packaged app, judged by the measurement's own analyzer: two axes in two successive
    windows of one process with unique generation identities and valid frames in every window that scrolls; a real draw that
    crosses the content extent with the requested logical distance, the expected effective offset and the offset the geometry
    drew at all recorded and equal; and a deliberate coordinate mismatch (the content drawn 7 points from where it was
    requested) that is refused as CONTENT_MISMATCH and yields no frames per second."""
    database = measure_gantt.Datasets(directory).store('B100')
    arguments = ['--database', str(database), '--page', 'gantt', '--measure-script', 'scroll-repeat', '--measure-seconds', '3']
    run = measure_gantt.Launched(bundle, directory, 'scroll-guard', arguments)
    try:
        assert run.wait_event('driver_done', 180) is not None, 'the scroll driver did not finish'
        run.quit()
        windows = measure_gantt.analyze_bf5(run.events, 3.0, run_id=run.measure_id)
    finally:
        run.close()
    assert not windows['invalid'], f"the real scroll log is INVALID: {windows['invalid']}: {[(w['axis'], w['window_id'], w['invalid']) for w in windows['windows']]}"
    found = windows['windows']
    assert len(found) == 4 and len({(w['window_id'], w['axis']) for w in found}) == 4, f'two axes in two windows need four unique windows: {[(w["window_id"], w["axis"]) for w in found]}'
    scrolling = [w for w in found if w['applicable']]
    assert scrolling and all(w['drawn'] > 0 and w['frames_per_second'] for w in scrolling), f'every scrolling window must have valid frames: {scrolling}'
    assert {w['axis'] for w in scrolling} >= {'vertical'} and len({w['window_id'] for w in scrolling if w['axis'] == 'vertical'}) == 2, 'the vertical axis must be measured in two successive windows'
    frames = windows['raw']['frames']
    crossed = [f for f in frames if f['counted'] and f['logical_distance'] > f['extent'] - f['viewport']]
    assert crossed, 'no frame crossed the content extent, so the wrap was not shown'
    assert all(abs(f['geometry_drawn_offset'] - f['expected_effective_offset']) < 1e-3 and f['geometry_drawn_offset'] < f['extent'] - f['viewport'] for f in crossed), 'a frame past the extent was drawn at another offset than expected'
    assert all(f['geometry_drawn_offset'] != f['logical_distance'] for f in crossed), 'a frame past the extent reported the requested distance as its drawn offset'
    shifted = measure_gantt.Launched(bundle, directory, 'scroll-displaced', [*arguments[:-4], '--measure-script', 'scroll', '--measure-seconds', '2', '--measure-displace', '7'])
    try:
        assert shifted.wait_event('driver_done', 120) is not None, 'the displaced scroll driver did not finish'
        shifted.quit()
        refused = measure_gantt.analyze_bf5(shifted.events, 2.0, run_id=shifted.measure_id)
    finally:
        shifted.close()
    assert 'CONTENT_MISMATCH' in refused['invalid'] and refused['values'] is None and all(w['frames_per_second'] is None for w in refused['windows']), f"a displaced draw must be INVALID/CONTENT_MISMATCH with no frames per second: {refused['invalid']}"
    assert shifted.by_name.get('frame_mismatch') and not shifted.by_name.get('frame'), 'a displaced draw must be logged as a mismatch and complete no frame'
    return (f"the packaged scroll log has {len(found)} unique windows over two axes ({len(scrolling)} scrolling, {sum(1 for w in found if not w['applicable'])} whose content fits its viewport and so has no scroll FPS), "
            f"{sum(w['drawn'] for w in scrolling)} valid frames, {len(crossed)} of them past the content extent with requested, expected and drawn offsets recorded and equal; "
            f"content displaced by 7 points is refused as {refused['invalid']} with no frames per second")


def _inside(frame, area, tolerance=1.5):
    x, y, w, h = frame
    ax, ay, aw, ah = area
    return x >= ax - tolerance and y >= ay - tolerance and x + w <= ax + aw + tolerance and y + h <= ay + ah + tolerance


DEFAULT_CONTENT = (1180, 760)  # the default window; its usable content area is 52 points shorter (the toolbar)


def _await_layout(app, problem_of, what, timeout=30):
    """The first report for which `problem_of` finds nothing wrong. It is evaluated on every state the app writes
    (a layout change, or the viewport's row count changing, asks the app for a write), and at the deadline the
    case fails, naming the condition that still did not hold."""
    last = ['no state was written']

    def ready(s):
        last[0] = problem_of(s)
        return last[0] is None
    try:
        return app.wait(ready, timeout, what)
    except AssertionError as error:
        raise AssertionError(f'{error}; the layout condition that did not hold: {last[0]}') from None


def _fresh_problem(app, state, launched):
    """The layout in this report was measured by this process after it was launched."""
    layout = state['gantt']['layout']
    if state['pid'] != app.process.pid:
        return f"the report is from process {state['pid']}, not {app.process.pid}"
    if not layout or layout['measured_at_ms'] < launched:
        return 'no layout has been measured since the launch yet'
    return None


def _overlap(a, b, tolerance=1.5):
    return min(a[0] + a[2], b[0] + b[2]) - max(a[0], b[0]) > tolerance and min(a[1] + a[3], b[1] + b[3]) - max(a[1], b[1]) > tolerance


def _size_problem(state, size):
    """The window facts, each in its own basis: the content view's bounds are the requested size, the outer
    window frame is not smaller, and the usable content (the layout rect, converted into the content view's
    coordinates) lies inside the bounds."""
    layout = state['gantt']['layout']
    bounds, outer, usable = layout['content_bounds'], layout['window_frame'], layout['usable_content']
    if not (bounds and outer and usable):
        return 'the window has not been measured yet'
    if abs(bounds[2] - size[0]) > 2 or abs(bounds[3] - size[1]) > 2:
        return f'the content view bounds {bounds[2:]} are not the requested {list(size)} yet'
    if outer[2] < bounds[2] - 1.5 or outer[3] < bounds[3] - 1.5:
        return f'the outer window frame {outer[2:]} is smaller than its content bounds {bounds[2:]}'
    if usable[2] <= 0 or usable[3] <= 0 or not _inside(usable, bounds):
        return f'the usable content {usable} is empty or outside the content bounds {bounds}'
    return None


def _gantt_problem(state, size, names):
    """What the Gantt page must show in the real window: every part measured and inside the usable content
    (the first row may overhang the row viewport only when the page is scrolled); the filter above the rows;
    the axis above the rows inside the body; the model's viewport row count the one the rows area shows."""
    gantt = state['gantt']
    if not gantt_loaded(state) or gantt['rows'] < 1:
        return 'the Gantt has no rows loaded yet'
    problem = _size_problem(state, size)
    if problem:
        return problem
    layout = gantt['layout']
    area, frames = layout['usable_content'], layout['frames']
    missing = [n for n in names if n not in frames]
    if missing:
        return f'{missing} have not been measured yet'
    where = f"window {layout['window_frame']}, content {layout['content_bounds']}, usable {area}, frames {frames}, viewport rows {gantt['viewport_rows']}"
    for name in names:
        if name != 'first_row' and not _inside(frames[name], area):
            return f'{name} lies outside the usable content area: {where}'
    if frames['sidebar'][3] < area[3] * 0.5:
        return f'the sidebar does not fill the window: {where}'
    toolbar, filt, strip, body, axis, rows, first = (frames[n] for n in ('toolbar', 'filter', 'strip', 'body', 'axis', 'rows', 'first_row'))
    if toolbar[1] + toolbar[3] > strip[1] + 1.5 or strip[1] + strip[3] > body[1] + 1.5:
        return f'the toolbar, the strip and the body are not in order, one under the other: {where}'
    if toolbar[3] <= 20 or strip[3] <= 20:
        return f'the toolbar or the strip has no height: {where}'
    if axis[1] < body[1] - 1.5 or rows[1] + rows[3] > body[1] + body[3] + 1.5 or axis[1] + axis[3] > rows[1] + 1.5:
        return f'the axis and the rows are not inside the body, the axis above the rows: {where}'
    fit = int(rows[3] // gantt['row_height'])
    if gantt['viewport_rows'] != fit:
        return f'the model counts {gantt["viewport_rows"]} viewport rows but the rows area shows {fit}: {where}'
    # The hosted filter field: a real field inside the toolbar, above the rows.
    if filt[2] < 50 or filt[3] < 10 or not _inside(filt, toolbar):
        return f'the filter field {filt} is not a visible field inside the toolbar {toolbar}: {where}'
    if filt[1] + filt[3] > rows[1] + 1.5 or filt[1] + filt[3] > first[1] + 1.5:
        return f'the filter field {filt} is not above the rows {rows} and the first row {first}: {where}'
    # The first row the viewport draws: a real row of the full row height, in the row viewport (a scrolled page
    # may have it partly above the viewport, so then it only has to meet it).
    if first[2] < 20 or abs(first[3] - gantt['row_height']) > 1.5:
        return f'the first row {first} is not a row of height {gantt["row_height"]}: {where}'
    if gantt['pan'][1] == 0:
        if not _inside(first, rows):
            return f'the first row {first} is not inside the row viewport {rows}: {where}'
    elif not _overlap(first, rows):
        return f'the first row {first} does not meet the row viewport {rows}: {where}'
    return None


def gantt_window_layout(bundle, directory):
    """The layout cases below, then the window left at its default size: the window frame is saved between
    launches, so a case at another size would otherwise open every later launch at that size."""
    try:
        return _window_layout_cases(bundle, directory)
    finally:
        eight = directory / 'layout-8.sqlite'
        if eight.exists():
            launched = now_ms()
            app = App(bundle, directory, ['--database', str(eight), '--page', 'now', '--window-size', 'x'.join(map(str, DEFAULT_CONTENT))])
            try:
                # The window reports its content at the default size, measured after the launch: the resize has happened.
                _await_layout(app, lambda s: _fresh_problem(app, s, launched) or _size_problem(s, DEFAULT_CONTENT), 'the default window size to be restored')
                app.quit()
            finally:
                app.close()


def _window_layout_cases(bundle, directory):
    """UI-LAYOUT-01: in the real, normal window the Gantt page takes a finite height from the window. The
    app reports the frames its own views really have (measured in the live window, not a rendered page),
    and each fixed part (sidebar, toolbar and filter, Schedule basis strip, axis, row viewport) must lie in
    the window's content area, in order, with the viewport row count equal to what the height shows. An
    unfixed page, whose root asks for about 2600 points, puts the body and the rows far below the area
    (and the sidebar, centred on it, above it) and counts about ninety rows, so every case below fails.
    The window is opened at the default window's size and at other sizes, each through `--window-size`
    (never relying on a saved frame); a drag while running is not driven here. The strip's complete text may be taller than its viewport (it scrolls)."""
    small = measure_gantt.write_plan(directory / 'layout-8.json', measure_gantt.fx_gantt())
    eight = directory / 'layout-8.sqlite'
    run_cli(eight, 'import', str(small))
    dense = directory / 'layout-dense.sqlite'
    run_cli(dense, 'import', str(measure_gantt.write_plan(directory / 'layout-dense.json', measure_gantt.dataset(100, 'branch'))))
    # The shell alone: on the Now page the root, the status bar, the sidebar and the page lie in the content area.
    launched = now_ms()
    app = App(bundle, directory, ['--database', str(eight), '--page', 'now', '--window-size', 'x'.join(map(str, DEFAULT_CONTENT))])
    try:
        shell = ('sidebar', 'root', 'status', 'page')

        def shell_problem(s):
            problem = _fresh_problem(app, s, launched) or _size_problem(s, DEFAULT_CONTENT)
            if problem or not connected(s):
                return problem or 'not connected'
            layout = s['gantt']['layout']
            missing = [n for n in shell if n not in layout['frames']]
            if missing:
                return f'{missing} not measured yet'
            outside = [n for n in shell if not _inside(layout['frames'][n], layout['usable_content'])]
            return f"{outside} lie outside the usable content area: {layout}" if outside else None
        _await_layout(app, shell_problem, 'the real window layout of the Now page')
        assert app.quit() == 0
    finally:
        app.close()
    names = ('sidebar', 'toolbar', 'filter', 'strip', 'axis', 'rows', 'body', 'first_row')
    # Window sizes: 900x612 is the declared minimum (a 900x560 content area under the 52-point toolbar).
    cases = [(eight, DEFAULT_CONTENT), (eight, (900, 612)), (dense, DEFAULT_CONTENT), (dense, (1000, 690)), (dense, (900, 612))]
    summary = []
    heights = {}
    for database, size in cases:
        launched = now_ms()
        arguments =['--database', str(database), '--page', 'gantt', '--window-size', f'{size[0]}x{size[1]}']
        app = App(bundle, directory, arguments)
        label = f"{database.stem} in a {size[0]}x{size[1]} window" + (' (the default size)' if size == DEFAULT_CONTENT else '')
        try:
            # The state is taken when every layout condition holds in a report written after the launch, by this
            # process; otherwise the case fails at the deadline with the condition that did not hold.
            state = _await_layout(app, lambda s: _fresh_problem(app, s, launched) or _gantt_problem(s, size, names), f'the real window layout of {label}')
            gantt = state['gantt']
            layout = gantt['layout']
            area, frames = layout['usable_content'], layout['frames']
            body, first = frames['body'], frames['first_row']
            heights[(database.stem, size)] = gantt['viewport_rows']
            assert app.quit() == 0
            summary.append(f"{label}: first row {round(first[3])}pt high at {round(first[1])}, filter at {round(frames['filter'][1])}, viewport {gantt['viewport_rows']} rows, body {round(body[3])}pt in a {round(area[3])}pt usable area")
        finally:
            app.close()
    assert heights[('layout-dense', (900, 612))] < heights[('layout-dense', (1000, 690))] < heights[('layout-dense', DEFAULT_CONTENT)], f'the viewport rows do not follow the window height: {heights}'
    return 'the real window layout (reported by the app from its own views, at launch sizes only; no drag): ' + '; '.join(summary)


# --------------------------------------------------------------------------------------------------
# The Detail as the real hosted view reports it: its accessibility attributes, its Tab position and its
# scroll offset. Every value below is read by the app from its own NSView / NSAccessibility objects and
# written under `host.detail` in the state file; none is inferred from `detail.sections` or the model.
# --------------------------------------------------------------------------------------------------

LONG_TITLE = ('Contract A with a long title that must be read in full, never cut for width. ' * 8)[:500]
LONG_OBJECTIVE = ' '.join(f'Objective sentence {n} states one more observable result that a reader must reach by keyboard alone.' for n in range(1, 12))[:966].rstrip()
LONG_CRITERIA = [(f'Criterion {n:02d} says in many words that the long acceptance text wraps over several lines in the Detail pane and stays whole. ' * 2).strip() for n in range(1, 13)]
# TEST-A's own estimate in the fixture and every direct relation it has in fx_gantt: kind, lag or lead, policy and both ends.
TEST_A_ESTIMATE = 'own estimate: optimistic 2.0 h, likely 4.0 h, pessimistic 6.0 h'
TEST_A_RELATIONS = ['FS (finish to start) TEST-A → TEST-B, no lag, Hard: always enforced',
                    'FS (finish to start) TEST-A → TEST-D, no lag, Hard: always enforced',
                    'SS (start to start) TEST-A → TEST-C, lag 2.0 h, Soft: may be waived by a recorded waiver',
                    'FF (finish to finish) TEST-A → TEST-E, lead 1.0 h, Hard: always enforced']
LONG_RELATIONS = [*TEST_A_RELATIONS, *[f'FS (finish to start) TEST-A → LONG-{n}, no lag, Hard: always enforced' for n in range(1, 9)]]
RELATION_TEXT = re.compile(r'^(FS|SS|FF|SF) \((finish|start) to (finish|start)\) ')


def long_detail_plan():
    """The Gantt fixture with TEST-A given a 500-character title, a 966-character objective, 12 long criteria and eight
    more direct relations beside its four."""
    plan = measure_gantt.fx_gantt()
    by_key = {w['key']: w for w in plan['work_items'].values()}
    first = by_key['TEST-A']
    first['title'] = LONG_TITLE
    first['contract']['objective'] = LONG_OBJECTIVE
    first['contract']['acceptance'] = [dict(text=text) for text in LONG_CRITERIA]
    for n in range(1, 9):
        extra = copy.deepcopy(by_key['TEST-D'])
        extra.update(id=str(uuid.UUID(int=(0x7 << 96) + (0xb0 << 8) + n)), key=f'LONG-{n}', title=f'Long relation target {n}', parent=None, order=[300 + n])
        extra['contract'] = dict(objective=f'Target {n} of the long relation list.', acceptance=[dict(text='Exists.')], capabilities=[], requirement_ids=[], assets=[])
        plan['work_items'][extra['id']] = extra
        plan['dependencies'].append(measure_gantt.edge(uuid.UUID(int=(91 << 96) + n), first['id'], extra['id']))
    return plan


def host_detail(state):
    return (state.get('host') or {}).get('detail')


def focus_events(state):
    """The Gantt region's own focus events (sequence, event, its window's actual first responder's class), as the region's
    host trace recorded them in its own window."""
    trace = (state.get('host') or {}).get('gantt_focus') or {}
    return [[e.get('seq'), e.get('event'), (e.get('first_responder') or {}).get('class')] for e in trace.get('events', [])]


def ax_strings(state):
    """Every title, label and value the real accessibility tree of the Detail scroll area carries, one entry per node."""
    detail = host_detail(state) or {}
    return [[node['title'], node['label'], node['value']] for node in detail.get('ax', [])]


def ax_missing(state, fragments):
    strings = ax_strings(state)
    return [f for f in fragments if not any(f in part for node in strings for part in node)]


def ax_count(state, fragment):
    """How many accessibility nodes carry the fragment: more than one would be an announcement said twice."""
    return sum(1 for node in ax_strings(state) if any(fragment in part for part in node))


def ax_relations(state):
    """Every relation text, in the Detail's words, that a node of the real accessibility tree carries."""
    return {part for node in ax_strings(state) for part in node if RELATION_TEXT.match(part)}


def await_ax(app, fragments, what, timeout=30):
    """The first state whose real accessibility tree holds every fragment; a failure names what is still missing."""
    try:
        return app.wait(lambda s: host_detail(s) is not None and not ax_missing(s, fragments), timeout, what)
    except AssertionError:
        if app.process.poll() is not None:
            raise
        last = app.state() or {}
        detail = host_detail(last)
        raise AssertionError(f'{what}: not in the real accessibility tree: {ax_missing(last, fragments) if detail else "(no host report of the Detail)"}; '
                             f'{len(ax_strings(last))} nodes seen of {(detail or {}).get("ax_visited")} visited, truncated={(detail or {}).get("ax_truncated")}; '
                             f'nodes {ax_strings(last)[:12]}') from None


def assert_relations(state, expected):
    """The tree carries exactly these relations, every one complete, none missing and none beside them, from a walk that was not cut short."""
    detail = host_detail(state)
    assert detail['ax_truncated'] is False, f'the accessibility walk was cut short after {detail["ax_visited"]} elements, so completeness cannot be judged'
    found = ax_relations(state)
    assert found == set(expected), f'the relations in the real accessibility tree are not the direct relations: missing {sorted(set(expected) - found)}, extra {sorted(found - set(expected))}'


def detail_accessibility(bundle, directory):
    """The Detail of TEST-A in the real window: the key, the three-point estimate and every direct dependency with its
    type, lag, policy and ids reach the accessibility tree as real text, each once, and no other relation does. It fails
    if the values are only held by a group that ignores its children (the defect), or are absent, or are announced twice."""
    database = gantt_store(directory, 'detail-ax')
    expected = ['TEST-A', 'optimistic 2.0 h', 'likely 4.0 h', 'pessimistic 6.0 h', 'FS (finish to start) TEST-A → TEST-B',
                'SS (start to start) TEST-A → TEST-C, lag 2.0 h, Soft: may be waived by a recorded waiver',
                'FF (finish to finish) TEST-A → TEST-E, lead 1.0 h, Hard: always enforced', TEST_A_ESTIMATE, *TEST_A_RELATIONS]
    app = App(bundle, directory, ['--database', str(database), '--page', 'detail', '--select-key', 'TEST-A'])
    try:
        state = await_ax(app, expected, 'the real Detail of TEST-A to expose its values')
        for fragment in expected[1:]:
            assert ax_count(state, fragment) == 1, f'{fragment!r} is announced by {ax_count(state, fragment)} nodes: {ax_strings(state)}'
        assert any(node[2] == 'TEST-A' or node[1] == 'TEST-A' or node[0] == 'TEST-A' for node in ax_strings(state)), 'the key is not a node of its own'
        assert_relations(state, TEST_A_RELATIONS)
        assert app.quit() == 0
        return (f'the real Detail of TEST-A exposed its key, its own three-point estimate and all {len(TEST_A_RELATIONS)} direct dependencies '
                f'(type, lag or lead, policy, ids) in {len(ax_strings(state))} accessibility nodes, each once and none beside them')
    finally:
        app.close()


def detail_scroll_route(bundle, directory):
    """The long-text Detail in the real window: the full 500-character title, the objective, the 12 criteria, the own
    estimate and all twelve direct relations (type, lag or lead, policy, ids) are complete in the accessibility tree and
    no other relation is; the Detail overflows its scroll area; and its content can be a key view. Keys: detail_keyboard_route."""
    database = directory / 'detail-long.sqlite'
    run_cli(database, 'import', str(measure_gantt.write_plan(directory / 'detail-long.json', long_detail_plan())))
    app = App(bundle, directory, ['--database', str(database), '--page', 'detail', '--select-key', 'TEST-A'])
    try:
        wanted = [LONG_OBJECTIVE, *[f'{n}. {text}' for n, text in enumerate(LONG_CRITERIA, 1)], TEST_A_ESTIMATE, *LONG_RELATIONS]
        app.wait(lambda s: host_detail(s) is not None and 'error' not in host_detail(s)['scroll'] and host_detail(s)['scroll']['max_offset'] > 0, 30,
                 'the long Detail to be laid out in its scroll area')
        state = await_ax(app, wanted, 'the long Detail to expose its objective, 12 criteria, estimate and relations')
        assert_relations(state, LONG_RELATIONS)
        # The title is the label of the Detail's header element, which the app's own walk of its scroll area does not read
        # (SwiftUI's elements give it no label in process); it is read from the app's accessibility tree as a client reads it.
        title = 'not checked'
        if not accessibility_trusted():
            not_assessed('detail_scroll_route (long title)', 'this process has no Accessibility permission, so the app\'s accessibility tree cannot be read as a client reads it')
        else:
            reader = None
            try:
                reader = Foreground(app.process.pid)
                found = reader.until(lambda: app.process.poll() is not None or reader.holds_text(LONG_TITLE), 15)
                assert app.process.poll() is None, f'the app exited ({app.process.returncode}) while its long title was read'
                assert found, f'the full {len(LONG_TITLE)}-character title is in no AXTitle, AXDescription or AXValue of the app\'s accessibility tree (tree notifications observed: {reader.tree_observed})'
                title = f'its full {len(LONG_TITLE)}-character title (read as a client reads it), '
            except NotAssessed as interruption:
                not_assessed('detail_scroll_route (long title)', interruption_or_death(app.process, interruption))
            finally:
                if reader:
                    reader.close()
        scroll = host_detail(state)['scroll']
        assert 'error' not in scroll and scroll['max_offset'] > 0, f'the long Detail does not overflow its scroll area, so there is nothing to scroll: {scroll}'
        # The window builds its key view loop when Tab is pressed (it recalculates it itself), so the loop cannot be walked
        # before that; what is read here is that the real view is able to join it. The Tab sequence itself is detail_keyboard_route.
        assert host_detail(state)['can_become_key_view'] is True and host_detail(state)['autorecalculates_key_view_loop'] is True, (
            f'the Detail content cannot be a key view of a window that rebuilds its loop, so Tab cannot reach it: {host_detail(state)["key_view_loop"]}')
        assert app.quit() == 0
    finally:
        app.close()
    return (f'the long Detail exposed {"" if title == "not checked" else title}its objective, {len(LONG_CRITERIA)} criteria, its own estimate and all '
            f'{len(LONG_RELATIONS)} direct relations (type, lag or lead, policy, ids) in the real accessibility tree and no other relation, overflows its '
            f'scroll area by {scroll["max_offset"]:.0f} pt and can be a key view (keyboard route reported on its own)')


# --------------------------------------------------------------------------------------------------
# The keyboard route through the long Detail. A key is sent only while the owned app is the frontmost
# application and its target window is the key window, as the system's accessibility API reports them for
# that process; a wait ends on an event (the app's state file, or the app's own activation and focus
# notifications) or at its deadline. When that guard stops holding the route stops there, NOT ASSESSED:
# no further key is sent and focus is never taken back mid-route. A deactivation or loss of the target window observed
# after the initial activation keeps the route interrupted even when the app is activated again, and an app that has
# exited is a failure, never an interruption.
# --------------------------------------------------------------------------------------------------

NOT_ASSESSED = []


class NotAssessed(Exception):
    """The keyboard route stopped because the environment no longer let it run (the owned app not frontmost, its
    window not key, no permission). Neither a pass nor a product failure."""


def not_assessed(case, reason):
    NOT_ASSESSED.append(f'{case}: {reason}')
    print(f'NOT ASSESSED (not a pass, not a product failure): {case}: {reason}', flush=True)


class Foreground:
    """Whether one owned process is frontmost with its target window key, read with the accessibility API from that
    process, and its activation and focus notifications, delivered by an AXObserver on this thread's run loop."""
    TARGET_WINDOW = 'main'
    NOTIFICATIONS = ('AXApplicationActivated', 'AXApplicationDeactivated', 'AXFocusedWindowChanged', 'AXMainWindowChanged', 'AXFocusedUIElementChanged')
    # Changes of the tree itself, observed where the app supports them, to read the tree again when it changes.
    TREE_NOTIFICATIONS = ('AXCreated', 'AXUIElementDestroyed', 'AXLayoutChanged', 'AXValueChanged', 'AXTitleChanged')
    UTF8 = 0x08000100

    def __init__(self, pid):
        ax = self.ax = ctypes.CDLL('/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices')
        cf = self.cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
        pointer = ctypes.c_void_p
        callback = ctypes.CFUNCTYPE(None, pointer, pointer, pointer, pointer)
        ax.AXUIElementCreateApplication.restype, ax.AXUIElementCreateApplication.argtypes = pointer, [ctypes.c_int]
        ax.AXUIElementCopyAttributeValue.restype, ax.AXUIElementCopyAttributeValue.argtypes = ctypes.c_int32, [pointer, pointer, ctypes.POINTER(pointer)]
        ax.AXUIElementSetAttributeValue.restype, ax.AXUIElementSetAttributeValue.argtypes = ctypes.c_int32, [pointer, pointer, pointer]
        ax.AXObserverCreate.restype, ax.AXObserverCreate.argtypes = ctypes.c_int32, [ctypes.c_int, callback, ctypes.POINTER(pointer)]
        ax.AXObserverAddNotification.restype, ax.AXObserverAddNotification.argtypes = ctypes.c_int32, [pointer, pointer, pointer, pointer]
        ax.AXObserverGetRunLoopSource.restype, ax.AXObserverGetRunLoopSource.argtypes = pointer, [pointer]
        cf.CFRunLoopGetCurrent.restype = pointer
        cf.CFRunLoopAddSource.argtypes = cf.CFRunLoopRemoveSource.argtypes = [pointer, pointer, pointer]
        cf.CFRunLoopRunInMode.restype, cf.CFRunLoopRunInMode.argtypes = ctypes.c_int32, [pointer, ctypes.c_double, ctypes.c_bool]
        cf.CFStringCreateWithCString.restype, cf.CFStringCreateWithCString.argtypes = pointer, [pointer, ctypes.c_char_p, ctypes.c_uint32]
        cf.CFStringGetLength.restype, cf.CFStringGetLength.argtypes = ctypes.c_long, [pointer]
        cf.CFStringGetCString.restype, cf.CFStringGetCString.argtypes = ctypes.c_bool, [pointer, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32]
        cf.CFGetTypeID.restype, cf.CFGetTypeID.argtypes = ctypes.c_ulong, [pointer]
        cf.CFStringGetTypeID.restype = cf.CFBooleanGetTypeID.restype = ctypes.c_ulong
        cf.CFBooleanGetValue.restype, cf.CFBooleanGetValue.argtypes = ctypes.c_bool, [pointer]
        cf.CFArrayGetTypeID.restype = ctypes.c_ulong
        cf.CFArrayGetCount.restype, cf.CFArrayGetCount.argtypes = ctypes.c_long, [pointer]
        cf.CFArrayGetValueAtIndex.restype, cf.CFArrayGetValueAtIndex.argtypes = pointer, [pointer, ctypes.c_long]
        cf.CFRelease.argtypes = [pointer]
        cf.CFEqual.restype, cf.CFEqual.argtypes = ctypes.c_bool, [pointer, pointer]
        ax.AXValueGetValue.restype, ax.AXValueGetValue.argtypes = ctypes.c_bool, [pointer, ctypes.c_int, pointer]
        self.mode = pointer.in_dll(cf, 'kCFRunLoopDefaultMode').value
        self.true = pointer.in_dll(cf, 'kCFBooleanTrue').value
        self.strings = {}
        self.events = []
        self.source = None
        # Set once by `arm`, after the initial activation; from then on the first observed interruption is kept.
        self.armed = False
        self.interrupted = None
        self.app = ax.AXUIElementCreateApplication(pid)
        self.callback = callback(lambda _observer, _element, name, _refcon: self.observed(self.text_of(name)))
        observer = pointer()
        error = ax.AXObserverCreate(pid, self.callback, ctypes.byref(observer))
        if error:
            raise NotAssessed(f'no accessibility observer could be made for the app (AXError {error})')
        self.observer = observer.value
        for name in self.NOTIFICATIONS:
            error = ax.AXObserverAddNotification(self.observer, self.app, self.string(name), None)
            if error:
                raise NotAssessed(f'the app\'s {name} notification could not be observed (AXError {error})')
        self.tree_observed = [name for name in self.TREE_NOTIFICATIONS if not ax.AXObserverAddNotification(self.observer, self.app, self.string(name), None)]
        self.source = ax.AXObserverGetRunLoopSource(self.observer)
        cf.CFRunLoopAddSource(cf.CFRunLoopGetCurrent(), self.source, self.mode)

    def close(self):
        if self.source:
            self.cf.CFRunLoopRemoveSource(self.cf.CFRunLoopGetCurrent(), self.source, self.mode)
            self.source = None
        for held in [getattr(self, 'observer', None), self.app, *self.strings.values()]:
            if held:
                self.cf.CFRelease(held)
        self.observer = self.app = None
        self.strings = {}

    def string(self, text):
        if text not in self.strings:
            self.strings[text] = self.cf.CFStringCreateWithCString(None, text.encode(), self.UTF8)
        return self.strings[text]

    def text_of(self, value):
        if not value or self.cf.CFGetTypeID(value) != self.cf.CFStringGetTypeID():
            return None
        size = self.cf.CFStringGetLength(value) * 4 + 1
        buffer = ctypes.create_string_buffer(size)
        return buffer.value.decode() if self.cf.CFStringGetCString(value, buffer, size, self.UTF8) else None

    def attribute(self, element, name):
        """(AXError, the value as a Python bool or str, or None) of one attribute of one element."""
        value = ctypes.c_void_p()
        error = self.ax.AXUIElementCopyAttributeValue(element, self.string(name), ctypes.byref(value))
        if error or not value.value:
            return error, None
        try:
            if self.cf.CFGetTypeID(value.value) == self.cf.CFBooleanGetTypeID():
                return 0, bool(self.cf.CFBooleanGetValue(value.value))
            return 0, self.text_of(value.value)
        finally:
            self.cf.CFRelease(value.value)

    def observed(self, name):
        """One notification of the app. Once the route is armed, an observed deactivation, or a window change that leaves
        the target window without the keyboard, interrupts it for good: a later activation does not undo it."""
        self.events.append(name)
        if self.armed and self.interrupted is None:
            if name == 'AXApplicationDeactivated':
                self.interrupted = 'AXApplicationDeactivated was observed'
            elif name in ('AXFocusedWindowChanged', 'AXMainWindowChanged'):
                lost = self.window_problem()
                if lost:
                    self.interrupted = f'{name} was observed and {lost}'

    def drain(self):
        """Handle every notification already queued on this thread's run loop, without waiting for more."""
        for _ in range(10000):
            if self.cf.CFRunLoopRunInMode(self.mode, 0, True) != 4:  # kCFRunLoopRunHandledSource
                return

    def arm(self):
        """Start the route, after its initial activation: what was queued before is handled unarmed, and from here an
        observed interruption is kept until this observer is closed."""
        self.drain()
        self.armed = True

    def interruption(self):
        """The interruption observed since the route was armed, after handling the queued notifications; or None."""
        self.drain()
        if self.interrupted:
            return f'{self.interrupted} during the route, which stays interrupted even if the app has been activated again since'
        return None

    def problem(self):
        """None while the route has observed no interruption and the app is frontmost with its target window key;
        otherwise what does not hold."""
        return self.interruption() or self.current_problem()

    def current_problem(self):
        """None while the app is frontmost and its target window is the key window; otherwise what does not hold."""
        error, frontmost = self.attribute(self.app, 'AXFrontmost')
        if frontmost is not True:
            return f'the owned app is not frontmost (AXFrontmost {frontmost}, AXError {error})'
        return self.window_problem()

    def window_problem(self):
        """None while the app's key window is its target window; otherwise what holds instead."""
        window = ctypes.c_void_p()
        error = self.ax.AXUIElementCopyAttributeValue(self.app, self.string('AXFocusedWindow'), ctypes.byref(window))
        if error or not window.value:
            return f'the owned app has no key window (AXFocusedWindow AXError {error})'
        try:
            role, identifier = self.attribute(window.value, 'AXRole')[1], self.attribute(window.value, 'AXIdentifier')[1]
        finally:
            self.cf.CFRelease(window.value)
        if role != 'AXWindow' or identifier != self.TARGET_WINDOW:
            return f'the key window is not the target window (role {role}, identifier {identifier})'
        return None

    def focused_identifier(self):
        """The accessibility identifier of the element that has the keyboard in the app, or None."""
        element = ctypes.c_void_p()
        if self.ax.AXUIElementCopyAttributeValue(self.app, self.string('AXFocusedUIElement'), ctypes.byref(element)) or not element.value:
            return None
        try:
            return self.attribute(element.value, 'AXIdentifier')[1]
        finally:
            self.cf.CFRelease(element.value)

    def element_value(self, element, name):
        """One attribute of one element that is itself a CF object (an element, an array, an AXValue), retained: the
        caller releases it. None if the attribute has no value."""
        value = ctypes.c_void_p()
        if self.ax.AXUIElementCopyAttributeValue(element, self.string(name), ctypes.byref(value)) or not value.value:
            return None
        return value.value

    def frame(self, element):
        """[x, y, width, height] of one element in screen points, from its AXPosition and AXSize; None if either is missing."""
        numbers = []
        for name, kind in (('AXPosition', 1), ('AXSize', 2)):  # kAXValueCGPointType, kAXValueCGSizeType
            value = self.element_value(element, name)
            if value is None:
                return None
            try:
                pair = (ctypes.c_double * 2)()
                if not self.ax.AXValueGetValue(value, kind, ctypes.byref(pair)):
                    return None
                numbers += list(pair)
            finally:
                self.cf.CFRelease(value)
        return numbers

    def describe(self, element):
        return {'role': self.attribute(element, 'AXRole')[1], 'identifier': self.attribute(element, 'AXIdentifier')[1], 'frame': self.frame(element)}

    def focused_chain(self, limit=20):
        """The element that has the keyboard in the app, then each AXParent up to the application: the role, identifier
        and frame of each, read now. [] when the app names no focused element."""
        chain = []
        element = self.element_value(self.app, 'AXFocusedUIElement')
        while element is not None:
            try:
                chain.append(self.describe(element))
                parent = self.element_value(element, 'AXParent') if len(chain) < limit else None
            finally:
                self.cf.CFRelease(element)
            element = parent
        return chain

    def find(self, identifier, limit=6000):
        """The role, identifier and frame of the first element of the app's accessibility tree, walked through AXChildren
        from the application, whose AXIdentifier is `identifier`; or None."""
        visited = 0

        def walk(element, depth):
            nonlocal visited
            visited += 1
            if visited > limit or depth > 60:
                return None
            if self.attribute(element, 'AXIdentifier')[1] == identifier:
                return self.describe(element)
            children = self.element_value(element, 'AXChildren')
            if children is None:
                return None
            try:
                if self.cf.CFGetTypeID(children) != self.cf.CFArrayGetTypeID():
                    return None
                for i in range(self.cf.CFArrayGetCount(children)):
                    found = walk(self.cf.CFArrayGetValueAtIndex(children, i), depth + 1)
                    if found:
                        return found
                return None
            finally:
                self.cf.CFRelease(children)
        return walk(self.app, 0)

    def receiver(self, identifier, limit=6000):
        """The app's actual keyboard receiver against the element of its focused window whose AXIdentifier is `identifier`, all
        read now: whether that window is the target window; whether the AXParent chain of AXFocusedUIElement reaches that same
        window; how many elements of that window carry the identifier; the first such element (role, identifier, frame); and
        whether the focused element is that element or a real AXParent descendant of it. Elements are compared with CFEqual,
        never by an identity remembered from an earlier view."""
        facts = {'target_window': False, 'focused_in_window': False, 'matches': 0, 'region': None, 'focused_in_region': False}
        window = self.element_value(self.app, 'AXFocusedWindow')
        if window is None:
            return facts
        chain = []
        try:
            facts['target_window'] = self.attribute(window, 'AXRole')[1] == 'AXWindow' and self.attribute(window, 'AXIdentifier')[1] == self.TARGET_WINDOW
            element = self.element_value(self.app, 'AXFocusedUIElement')
            while element is not None:
                chain.append(element)
                if self.cf.CFEqual(element, window):
                    facts['focused_in_window'] = True
                    break
                element = self.element_value(element, 'AXParent') if len(chain) < 40 else None
            below = chain[:-1] if facts['focused_in_window'] else chain
            visited = 0

            def walk(node, depth):
                nonlocal visited
                visited += 1
                if visited > limit or depth > 60:
                    return
                if self.attribute(node, 'AXIdentifier')[1] == identifier:
                    facts['matches'] += 1
                    if facts['matches'] == 1:
                        facts['region'] = self.describe(node)
                        facts['focused_in_region'] = any(self.cf.CFEqual(node, held) for held in below)
                children = self.element_value(node, 'AXChildren')
                if children is None:
                    return
                try:
                    if self.cf.CFGetTypeID(children) != self.cf.CFArrayGetTypeID():
                        return
                    for i in range(self.cf.CFArrayGetCount(children)):
                        walk(self.cf.CFArrayGetValueAtIndex(children, i), depth + 1)
                finally:
                    self.cf.CFRelease(children)
            walk(window, 0)
        finally:
            for held in chain:
                self.cf.CFRelease(held)
            self.cf.CFRelease(window)
        return facts

    def holds_text(self, fragment, limit=6000):
        """Whether an element of the app's accessibility tree, walked through AXChildren from the application, carries
        `fragment` in its AXTitle, AXDescription or AXValue."""
        visited = 0

        def walk(element, depth):
            nonlocal visited
            visited += 1
            if visited > limit or depth > 60:
                return False
            if any(fragment in text for text in (self.attribute(element, name)[1] for name in ('AXTitle', 'AXDescription', 'AXValue')) if isinstance(text, str)):
                return True
            children = ctypes.c_void_p()
            if self.ax.AXUIElementCopyAttributeValue(element, self.string('AXChildren'), ctypes.byref(children)) or not children.value:
                return False
            try:
                if self.cf.CFGetTypeID(children.value) != self.cf.CFArrayGetTypeID():
                    return False
                return any(walk(self.cf.CFArrayGetValueAtIndex(children.value, i), depth + 1) for i in range(self.cf.CFArrayGetCount(children.value)))
            finally:
                self.cf.CFRelease(children.value)
        return walk(self.app, 0)

    def until(self, condition, timeout):
        """Handle the app's notifications as they arrive until `condition()` holds or the deadline passes; whether it held."""
        deadline = time.monotonic() + timeout
        while not condition():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return False
            self.cf.CFRunLoopRunInMode(self.mode, remaining, True)
        return True

    def activate(self, process, timeout=15):
        """Ask for the app to be frontmost through the accessibility API and, if that is refused or does not settle, once
        through System Events, checking each request's result; wait on the app's activation and focus notifications until
        it is frontmost with its target window key. NotAssessed otherwise. Only before the route is armed: focus is never
        taken back mid-route."""
        assert not self.armed, 'the route is armed: the app is not made frontmost again mid-route'
        settled = lambda: process.poll() is not None or self.problem() is None  # noqa: E731
        error = self.ax.AXUIElementSetAttributeValue(self.app, self.string('AXFrontmost'), self.true)
        if error or not self.until(settled, 5):
            # The same request through System Events, its result checked as well; still before any key.
            asked = subprocess.run(['/usr/bin/osascript', '-e', f'tell application "System Events" to set frontmost of (first process whose unix id is {process.pid}) to true'],
                                   capture_output=True, text=True, timeout=15)
            if asked.returncode != 0:
                raise NotAssessed(f'the owned app could not be made frontmost: AXFrontmost request AXError {error}, then osascript exited {asked.returncode}: {asked.stderr.strip()[:300]}; no key was sent')
            if not self.until(settled, timeout):
                raise NotAssessed(f'within {timeout} s of accepted activation requests: {self.problem()}; no key was sent')
        if process.poll() is not None:
            raise AssertionError(f'the app exited ({process.returncode}) while it was being made frontmost')


def interruption_or_death(process, interruption):
    """The reason a route stopped, the owned process checked first: if it has exited, the stop was its death, whatever AX,
    activation or focus error came before, and that is a failure keeping its exit status and that error. Otherwise the
    environmental reason, NOT ASSESSED."""
    if process.poll() is not None:
        raise AssertionError(f'the app exited ({process.returncode}), a failure and not an interruption; when it was found: {interruption}') from interruption
    return str(interruption)


def uninterrupted(front, what):
    """Stop the route if an interruption was observed since it was armed, before `what` (a next key, a wait taken as
    satisfied, the route's result) counts."""
    interrupted = front.interruption()
    if interrupted:
        raise NotAssessed(f'before {what}: {interrupted}')


def guarded_key(app, front, step, code, flags=0):
    """Send one key only if the app still runs and is frontmost with its target window key; otherwise stop the route."""
    if app.process.poll() is not None:
        raise AssertionError(f'the app exited ({app.process.returncode}) before {step}')
    problem = front.problem()
    if problem:
        raise NotAssessed(f'before {step}: {problem}; no key was sent')
    post_key(app.process.pid, code, flags)


def guarded_wait(app, front, predicate, timeout, what):
    """`app.wait`, where a missed deadline after the guard stopped holding is an interruption, not a failure. A process
    exit, or a missed deadline while the guard still holds, stays a failure. A satisfied wait counts only if no
    interruption was observed meanwhile."""
    try:
        state = app.wait(predicate, timeout, what)
    except AssertionError:
        if app.process.poll() is None:
            problem = front.problem()
            if problem:
                raise NotAssessed(f'while waiting for {what}: {problem}') from None
        raise
    uninterrupted(front, f'{what} counts as shown')
    return state


def tab_to_content(app, front, round_name, limit=12):
    """Real Tab presses, each guarded, until the element with the keyboard in the app is the Detail content; after each,
    the app's focus notification (or its deadline) and then the focused element's identifier. The identifiers reached."""
    reached = []
    for press in range(1, limit + 1):
        before = front.events.count('AXFocusedUIElementChanged')
        guarded_key(app, front, f'Tab {press} of the {round_name} round', KEY_CODES['tab'])
        front.until(lambda: front.events.count('AXFocusedUIElementChanged') > before, 5)
        reached.append(front.focused_identifier())
        if reached[-1] == 'detail.content':
            return reached
    problem = front.problem()
    if problem:
        raise NotAssessed(f'after {limit} Tab presses of the {round_name} round: {problem}')
    last = host_detail(app.state() or {}) or {}
    raise AssertionError(f'{limit} real Tab presses, each sent with the app frontmost and its window key, never gave the Detail content the keyboard '
                         f'({round_name} round; focused after each: {reached}; host {last.get("first_responder")}, key window {last.get("key_window")}, loop {last.get("key_view_loop")})')


GANTT_REGION = 'gantt.region'


def gantt_receiver(chain, row, receiver):
    """Whether the actual AX focused element (`chain`, from `Foreground.focused_chain`) is the Gantt's native keyboard region:
    by `receiver` (from `Foreground.receiver(GANTT_REGION)`), the focused element is the one element of the owned app's focused
    target window whose AXIdentifier is `gantt.region`, or a real AXParent descendant of it, and its AXParent chain reaches that
    same window. Still excluded on their own: the window, the window's root group (whose parent is the window, shared with the
    sidebar), the split view, the filter field and the Detail content; the receiver must be below the split view, and the
    region's frame must hold the AX row `row` (from `Foreground.find`). No identity of an AX object of an earlier view is needed."""
    if len(chain) < 2 or not row or not row.get('frame'):
        return False
    region = receiver.get('region') or {}
    if not (receiver.get('target_window') and receiver.get('focused_in_window') and receiver.get('matches') == 1 and receiver.get('focused_in_region')
            and region.get('identifier') == GANTT_REGION and region.get('frame')):
        return False
    focused, parent = chain[0], chain[1]
    if focused['role'] in ('AXApplication', 'AXWindow', 'AXSplitGroup', 'AXTextField') or focused['identifier'] in ('detail.content', 'gantt.filter') or parent['role'] == 'AXWindow':
        return False
    if not any(node['role'] == 'AXSplitGroup' for node in chain[1:]):
        return False
    x, y, width, height = region['frame']
    rx, ry, rwidth, rheight = row['frame']
    return x <= rx + rwidth / 2 <= x + width and y <= ry + rheight / 2 <= y + height


def return_focus(app, front, name, row):
    """After `name` closed Detail, before any key: wait, on the app's state writes and then on its accessibility
    notifications, for (a) the reloaded Gantt, (b) the originating row in the state and in the AX tree with the cursor on
    it and the Detail content gone, and (c) the Gantt's own region as the actual AX focused element; then, on the state
    writes, for the focus the view reports, a sync aid only. The state and the AX facts read. If they do not all hold while
    the guard holds, a failure saying the return focus was NOT ESTABLISHED, with the complete actual state; no key is sent."""
    key = row[len('gantt.row.'):]

    def shown(s):
        g = s['gantt']
        return s['page'] == 'gantt' and gantt_loaded(s) and key in g.get('keys', []) and g['cursor_key'] == key and g['focus_target'] == row and host_detail(s) is None

    def read_ax():
        return {'focused': front.focused_chain(), 'receiver': front.receiver(GANTT_REGION), 'row': front.find(row), 'detail_content': front.find('detail.content')}

    def ax_ready(facts):
        return facts['row'] is not None and facts['detail_content'] is None and gantt_receiver(facts['focused'], facts['row'], facts['receiver'])

    def receiving():
        facts.update(read_ax())
        return ax_ready(facts)

    facts, unmet = {}, None
    try:
        guarded_wait(app, front, shown, 20, f'{name} to return to the reloaded Gantt with the cursor on the originating row')
    except AssertionError:
        if app.process.poll() is not None:
            raise
        unmet = '(a) the reloaded Gantt or (b) the originating row with the cursor on it and the Detail content gone, in the state'
    if unmet is None and not front.until(receiving, 10):
        unmet = '(b) the originating row present and the Detail content gone, or (c) the Gantt\'s own region as the AX focused element, in the AX tree'
    if unmet is None:
        uninterrupted(front, f'the AX focus after {name} counts as the Gantt\'s')
        try:
            guarded_wait(app, front, lambda s: shown(s) and s['gantt']['focus_reported'] == row, 10, f'the Gantt view to report the row\'s focus after {name}')
        except AssertionError:
            if app.process.poll() is not None:
                raise
            unmet = 'the focus the Gantt view reports (the sync aid)'
    state = app.state() or {}
    facts = read_ax()
    if unmet is None and not (shown(state) and state['gantt']['focus_reported'] == row and ax_ready(facts)):
        unmet = 'the final reading of (a), (b), (c) and the reported focus together'
    if unmet:
        g = state.get('gantt') or {}
        actual = {'page': state.get('page'), 'loaded': g.get('loaded'), 'rows': g.get('rows'), 'origin_in_keys': key in (g.get('keys') or []), 'cursor': g.get('cursor_key'),
                  'focus_target': g.get('focus_target'), 'focus_reported': g.get('focus_reported'), 'host': state.get('host') or {},
                  'ax_focused_chain': facts['focused'], 'ax_receiver': facts['receiver'], 'ax_origin_row': facts['row'], 'ax_detail_content': facts['detail_content'],
                  'guard': 'holding: frontmost, target window key', 'latch': front.interrupted, 'armed': front.armed}
        problem = front.problem()
        if problem:
            # Still NOT ASSESSED; the state and the host's own focus events read at that moment are kept, as context only.
            actual['guard'] = f'not holding: {problem}'
            raise NotAssessed(f'while waiting for the return focus after {name}: {problem}; no Down was sent; {unmet} did not hold; actual: {json.dumps(actual, sort_keys=True)}')
        if app.process.poll() is not None:
            raise AssertionError(f'the app exited ({app.process.returncode}) while waiting for the return focus after {name}')
        raise AssertionError(f'the return focus after {name} was NOT ESTABLISHED (neither a Down failure nor an environmental loss; the guard holds and no Down '
                             f'was sent): {unmet} did not hold; actual: {json.dumps(actual, sort_keys=True)}')
    uninterrupted(front, f'the return focus after {name} counts as established')
    return state, facts


def detail_keyboard_route(bundle, directory):
    """The long Detail by keyboard, in one continuous guarded route: real key events (CGEventPostToPid) move the Gantt
    cursor to TEST-A, Return opens Detail, Tab gives its content the keyboard, Page Down, End and Home scroll it as the real
    scroll view reports it, Escape (then, in a second round, ⌘[) returns to the originating row, and real Down and Up
    arrows move the cursor off it and back. Only the complete route passes; an interrupted one is NOT ASSESSED."""
    case = 'detail_keyboard_route'
    if not accessibility_trusted():
        not_assessed(case, 'this process has no Accessibility permission (AXIsProcessTrusted is false), so CGEventPostToPid would be ignored; no key was sent')
        return None
    database = directory / 'detail-keys.sqlite'
    run_cli(database, 'import', str(measure_gantt.write_plan(directory / 'detail-keys.json', long_detail_plan())))
    work = run_cli(database, 'schedule')['work']
    order = [w['key'] for w in hierarchy_order(work)]
    target = order.index('TEST-A')
    app = App(bundle, directory, ['--database', str(database), '--page', 'gantt'])
    front = None
    try:
        app.wait(gantt_loaded, what='the Gantt rows of the long plan')
        front = Foreground(app.process.pid)
        front.activate(app.process)
        front.arm()
        guarded_key(app, front, 'the first Down arrow', KEY_CODES['down'])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] != order[0], 10, 'the first real Down arrow to move the cursor')
        for n in range(target - 1):
            guarded_key(app, front, f'Down arrow {n + 2} toward TEST-A', KEY_CODES['down'])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == 'TEST-A', 20, 'the cursor on TEST-A')
        row = 'gantt.row.TEST-A'
        reported = []
        for name, key, flags in (('Escape', KEY_CODES['escape'], 0), ('Command-[', 33, 0x100000)):
            guarded_key(app, front, f'Return of the {name} round', KEY_CODES['return'])
            opened = guarded_wait(app, front, lambda s: s['page'] == 'detail' and host_detail(s) is not None, 20, 'Return to open the long Detail')
            assert opened['gantt']['detail_return'] == row, opened['gantt']
            tab_to_content(app, front, name)
            reached = guarded_wait(app, front, lambda s: (host_detail(s) or {}).get('first_responder', {}).get('is_detail_content') is True and host_detail(s)['key_window'] is True,
                                   10, 'the app to report the Detail content as the first responder of its key window')
            assert host_detail(reached)['first_responder']['identifier'] == 'detail.content', host_detail(reached)['first_responder']

            def offset(s):
                return host_detail(s)['scroll']['offset']
            top = offset(reached)
            guarded_key(app, front, f'Page Down of the {name} round', KEY_CODES['page_down'])
            first = guarded_wait(app, front, lambda s: host_detail(s) and offset(s) > top + 1, 20, 'Page Down to scroll the Detail')
            guarded_key(app, front, f'the second Page Down of the {name} round', KEY_CODES['page_down'])
            second = guarded_wait(app, front, lambda s: host_detail(s) and offset(s) > offset(first) + 1, 20, 'a second Page Down to scroll further')
            guarded_key(app, front, f'End of the {name} round', KEY_CODES['end'])
            bottom = guarded_wait(app, front, lambda s: host_detail(s) and abs(offset(s) - host_detail(s)['scroll']['max_offset']) < 1, 20, 'End to reach the bottom of the Detail')
            guarded_key(app, front, f'Home of the {name} round', KEY_CODES['home'])
            guarded_wait(app, front, lambda s: host_detail(s) and offset(s) < 1, 20, 'Home to return to the top of the Detail')
            assert host_detail(second)['first_responder']['is_detail_content'] and host_detail(bottom)['first_responder']['is_detail_content'], f'the content lost the keyboard while scrolling: after Page Down {host_detail(first)["first_responder"]}, after the second {host_detail(second)["first_responder"]}, at the bottom {host_detail(bottom)["first_responder"]}'
            guarded_key(app, front, name, key, flags)
            # Exactly one Down, only once the reloaded Gantt, the originating row and the actual AX receiver are ready.
            back, facts = return_focus(app, front, name, row)
            assert back['gantt']['selected_key'] == 'TEST-A' or back['gantt']['cursor_key'] == 'TEST-A', back['gantt']
            # The row has the keyboard when real Down and Up arrows move the cursor off it and back. `focus_reported`, what the
            # Gantt view reports from its native region's actual first-responder state, is recorded beside the AX receiver as a sync aid.
            reported.append({'focus_reported': back['gantt']['focus_reported'], 'ax_focused': facts['focused'][:2], 'ax_region': facts['receiver']['region'],
                             'host_focus_events': focus_events(back)[-12:]})
            guarded_key(app, front, f'Down after {name}', KEY_CODES['down'])
            guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] not in (None, 'TEST-A'), 10, f'a real Down arrow to move the cursor off the row after {name}')
            guarded_key(app, front, f'Up after {name}', KEY_CODES['up'])
            guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == 'TEST-A', 10, f'a real Up arrow to bring the cursor back to the row after {name}')
        uninterrupted(front, 'the route is complete')
        if app.process.poll() is not None:
            raise AssertionError(f'the app exited ({app.process.returncode}) at the end of the keyboard route')
        assert app.quit() == 0
        return ('the complete guarded keyboard route: real key events (CGEventPostToPid), each sent with the app frontmost and its window key, reached the long '
                'Detail content by Tab, scrolled it with Page Down, End and Home as the real scroll view reports it, and returned to the originating Gantt row with '
                f'Escape and with Command-[, where, once the Gantt\'s own region was the actual AX focused element, one real Down and one Up moved the cursor '
                f'off the row and back (the reported focus and the AX receiver after those returns: {reported})')
    except NotAssessed as interruption:
        not_assessed(case, interruption_or_death(app.process, interruption))
        return None
    finally:
        if front:
            front.close()
        app.close()


FILTER = 'gantt.filter'


def filter_editor(state):
    """The host's own readings of the Gantt filter's field editor in its window: `current` (null while no editor edits the
    filter) and the bounded `readings`, each read from the real text view (marked text, ranges, string), never from the model."""
    return (state.get('host') or {}).get('gantt_filter_editor') or {}


def editor_holds(state, text):
    """The filter's real editor is its window's first responder and holds exactly `text`, none of it marked (uncommitted)."""
    current = filter_editor(state).get('current')
    return isinstance(current, dict) and current['first_responder'] is True and current['has_marked_text'] is False and current['string'] == text


def filter_focused(front):
    """(whether the app's AX focused element is the Gantt filter field, True / False, or None when that cannot be told; the
    focused element and its first two AXParents; the `Foreground.receiver(FILTER)` facts), read now. Only a valid reading
    counts either way: the focused window is the target window, the focused element's AXParent chain reaches it, and exactly
    one element of it carries `gantt.filter`; an empty chain or a failed read is None, never "left the filter". On the filter:
    the focused element is that one element or a real AXParent descendant of it; or, only then, an anonymous text field or
    text area (no AXIdentifier) inside that same element's frame, the field's real editor exposed beside it."""
    chain = front.focused_chain(limit=3)
    receiver = front.receiver(FILTER)
    field = receiver.get('region') or {}
    if not (chain and receiver['target_window'] and receiver['focused_in_window'] and receiver['matches'] == 1 and field.get('frame')):
        return None, chain, receiver
    if receiver['focused_in_region']:
        return True, chain, receiver
    focused = chain[0]
    within = False
    if focused.get('role') in ('AXTextField', 'AXTextArea') and not focused.get('identifier') and focused.get('frame'):
        (x, y, width, height), (fx, fy, fwidth, fheight) = field['frame'], focused['frame']
        within = x - 1.5 <= fx and y - 1.5 <= fy and fx + fwidth <= x + width + 1.5 and fy + fheight <= y + height + 1.5
    return within, chain, receiver


def ax_focus(app, front, on_filter, what, timeout=10):
    """Handle the app's accessibility notifications until a valid reading (see `filter_focused`) says the AX focused element
    is (`on_filter`) or is not the filter field; that element (role, identifier, frame). A missed deadline is a failure while
    the guard holds and NOT ASSESSED once it does not; an exited app fails."""
    seen = [None]

    def holds():
        seen[0] = filter_focused(front)
        return seen[0][0] == on_filter
    if not front.until(holds, timeout):
        if app.process.poll() is not None:
            raise AssertionError(f'the app exited ({app.process.returncode}) while waiting for {what}')
        problem = front.problem()
        if problem:
            raise NotAssessed(f'while waiting for {what}: {problem}')
        raise AssertionError(f'{what}: for {timeout} s, the app frontmost and its window key, the AX focused chain stayed {seen[0][1]} '
                             f'and the receiver of {FILTER!r} was {seen[0][2]}')
    uninterrupted(front, f'{what} counts as shown')
    return seen[0][1][0] if seen[0][1] else None


def filter_exit_route(bundle, directory):
    """Leaving the Gantt filter, in one continuous guarded route of real key events (CGEventPostToPid): / gives the filter the
    keyboard; c and Return leave a committed c in its real field editor; Tab moves the keyboard out of the field, Shift-Tab back
    into it and a second Shift-Tab out of it backwards, none of them naming a row (ordinary traversal is never made a row focus);
    Tab into it again, Command-A and Delete clear it, and Escape leaves it. Only once the editor is released, the rows loaded,
    the cursor row named and the Gantt's own region the actual unique AX focused element of the owned window, one real Down
    and one real Up move the cursor. Plain keys and Return are not an input-method composition: whether the host's real editor
    held marked text is reported, and no input-method pass is claimed."""
    case = 'filter_exit_route'
    if not accessibility_trusted():
        not_assessed(case, 'this process has no Accessibility permission (AXIsProcessTrusted is false), so CGEventPostToPid would be ignored; no key was sent')
        return None
    database = gantt_store(directory, 'filter-exit')
    app = App(bundle, directory, ['--database', str(database), '--page', 'gantt'])
    front = None

    def filtering(s, text):
        return s['gantt']['filter']['editing'] is True and s['gantt']['filter']['text'] == text

    def left(s, text):
        return s['gantt']['filter']['editing'] is False and s['gantt']['filter']['text'] == text

    def named_row(s):
        return (s['gantt']['focus_target'] or '').startswith('gantt.row.')
    try:
        order = app.wait(gantt_loaded, what='the Gantt rows')['gantt']['keys']
        front = Foreground(app.process.pid)
        front.activate(app.process)
        front.arm()
        guarded_key(app, front, 'the first Down arrow', KEY_CODES['down'])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] != order[0], 10, 'the first real Down arrow to move the cursor')
        guarded_key(app, front, '/ to the filter', KEY_CODES['slash'])
        guarded_wait(app, front, lambda s: filtering(s, '') and editor_holds(s, ''), 10, '/ to give the filter\'s real editor the keyboard')
        ax_focus(app, front, True, 'the filter as the AX focused element after /')
        guarded_key(app, front, 'c in the filter', KEY_CODES['c'])
        guarded_wait(app, front, lambda s: (filter_editor(s).get('current') or {}).get('string') == 'c', 10, 'c in the filter\'s real editor')
        guarded_key(app, front, 'Return to commit the filter text', KEY_CODES['return'])
        typed = guarded_wait(app, front, lambda s: filtering(s, 'c') and editor_holds(s, 'c'), 10, 'c committed by Return in the filter\'s real editor and the filter')
        readings = filter_editor(typed).get('readings') or []
        assert readings and filter_editor(typed)['window'] == (typed['host'].get('gantt_focus') or {}).get('window'), (
            f'the host has no reading of the filter\'s editor in the region\'s own window: {filter_editor(typed)}')
        marked = [r for r in readings if r.get('has_marked_text')]

        guarded_key(app, front, 'Tab out of the filter', KEY_CODES['tab'])
        after_tab = ax_focus(app, front, False, 'Tab to move the keyboard out of the filter')
        tabbed = guarded_wait(app, front, lambda s: left(s, 'c'), 10, 'the filter to say it lost the keyboard after Tab')
        assert not named_row(tabbed), f'Tab out of the filter named a row, which ordinary traversal must not: {tabbed["gantt"]}'
        guarded_key(app, front, 'Shift-Tab back into the filter', KEY_CODES['tab'], SHIFT_FLAG)
        ax_focus(app, front, True, 'Shift-Tab to give the filter the keyboard again')
        guarded_wait(app, front, lambda s: filtering(s, 'c') and editor_holds(s, 'c'), 10, 'the filter\'s real editor to have the keyboard again after Shift-Tab')
        guarded_key(app, front, 'Shift-Tab out of the filter, backwards', KEY_CODES['tab'], SHIFT_FLAG)
        before_filter = ax_focus(app, front, False, 'Shift-Tab to move the keyboard out of the filter backwards')
        back_tabbed = guarded_wait(app, front, lambda s: left(s, 'c'), 10, 'the filter to say it lost the keyboard after Shift-Tab')
        assert (before_filter or {}).get('identifier') != GANTT_REGION and not named_row(back_tabbed), (
            f'Shift-Tab backwards out of the filter reached {before_filter!r} with focus target {back_tabbed["gantt"]["focus_target"]!r}: ordinary traversal was made a row focus')
        guarded_key(app, front, 'Tab into the filter again', KEY_CODES['tab'])
        ax_focus(app, front, True, 'Tab to give the filter the keyboard again')
        guarded_wait(app, front, lambda s: filtering(s, 'c') and editor_holds(s, 'c'), 10, 'the filter\'s real editor to have the keyboard again after Tab')

        guarded_key(app, front, 'Command-A in the filter', KEY_CODES['a'], COMMAND_FLAG)
        guarded_wait(app, front, lambda s: editor_holds(s, 'c') and filter_editor(s)['current']['selected_range'] == [0, 1], 10, 'Command-A to select the text in the filter\'s real editor')
        guarded_key(app, front, 'Delete to clear the filter', KEY_CODES['delete'])
        cleared = guarded_wait(app, front, lambda s: filtering(s, '') and editor_holds(s, '') and gantt_loaded(s) and s['gantt']['rows'] == s['gantt']['total_rows'], 10,
                               'Delete to clear the filter in its real editor, every row shown')
        cursor = cleared['gantt']['cursor_key']
        row = f'gantt.row.{cursor}'
        guarded_key(app, front, 'Escape to leave the filter', KEY_CODES['escape'])
        # Ready only once the filter and its real editor have let the keyboard go; then the row and the actual AX receiver.
        guarded_wait(app, front, lambda s: left(s, '') and not (filter_editor(s).get('current') or {}).get('first_responder'), 10,
                     'the filter and its real editor to release the keyboard after Escape')
        back, facts = return_focus(app, front, 'Escape from the filter', row)
        assert left(back, '') and not (filter_editor(back).get('current') or {}).get('first_responder'), f'the filter took the keyboard back: {back["gantt"]["filter"]}, {filter_editor(back)}'
        keys = back['gantt']['keys']
        at = keys.index(cursor)
        first, second = ('down', 'up') if at + 1 < len(keys) else ('up', 'down')
        neighbour = keys[at + 1] if first == 'down' else keys[at - 1]
        # The exact facts the keys are sent on, captured before them.
        ready = {'cursor': cursor, 'focus_target': back['gantt']['focus_target'], 'focus_reported': back['gantt']['focus_reported'], 'filter': back['gantt']['filter'],
                 'editor': filter_editor(back).get('current'), 'ax_focused': facts['focused'][:2], 'ax_region': facts['receiver']['region'],
                 'host_focus_events': focus_events(back)[-12:]}
        guarded_key(app, front, f'{first} after Escape from the filter', KEY_CODES[first])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == neighbour, 10, f'a real {first} arrow to move the cursor off {cursor} after Escape from the filter')
        guarded_key(app, front, f'{second} after Escape from the filter', KEY_CODES[second])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == cursor, 10, f'a real {second} arrow to bring the cursor back to {cursor}')
        uninterrupted(front, 'the route is complete')
        if app.process.poll() is not None:
            raise AssertionError(f'the app exited ({app.process.returncode}) at the end of the filter route')
        assert app.quit() == 0
        composition = (f'the host\'s real editor held marked text in {len(marked)} of {len(readings)} readings before Return committed c (an input method composed it; '
                       'seen by the host only, not an input-method acceptance)' if marked else
                       f'the host\'s real editor held no marked text in any of its {len(readings)} readings: plain key presses and Return, NOT an input-method composition')
        return (f'the guarded filter route: / gave the filter the keyboard, c and Return committed c in its real editor ({composition}); Tab moved the keyboard to '
                f'{after_tab!r} and Shift-Tab backwards to {before_filter!r}, neither naming a row; Command-A and Delete cleared it and, after Escape, with the editor '
                f'released and the Gantt\'s own region the actual AX focused element, one real {first} and one {second} moved the cursor off {cursor} and back '
                f'(ready on: {ready})')
    except NotAssessed as interruption:
        not_assessed(case, interruption_or_death(app.process, interruption))
        return None
    finally:
        if front:
            front.close()
        app.close()


def filter_composition_route(bundle, directory):
    """Escape during an input method's composition in the Gantt filter, in one continuous guarded route of real key events
    (CGEventPostToPid, a virtual key and no text): / gives the filter the keyboard and c is typed with whatever input source is
    selected, never changed here. Only if the host's real field editor then holds marked text is this a composition: (a) one
    Escape is the input method's, and the editor keeps the keyboard with no marked text left, the field the actual AX focused
    element and the filter still editing; (b) c and Return commit text in the editor and the filter; (c) Command-A and Delete
    clear it and an ordinary Escape (no marked text) hands the keyboard to the Gantt's own region, where one real Down and one
    Up move the cursor. A plain keyboard (no marked text) is NOT ASSESSED: marked text is never made or faked here."""
    case = 'filter_composition_route'
    if not accessibility_trusted():
        not_assessed(case, 'this process has no Accessibility permission (AXIsProcessTrusted is false), so CGEventPostToPid would be ignored; no key was sent')
        return None
    database = gantt_store(directory, 'filter-composition')
    app = App(bundle, directory, ['--database', str(database), '--page', 'gantt'])
    front = None

    def current(s):
        return filter_editor(s).get('current') or {}

    def after(s, sequence):
        return (filter_editor(s).get('sequence') or 0) > sequence

    def editing(s):
        return s['gantt']['filter']['editing'] is True and current(s).get('first_responder') is True
    try:
        order = app.wait(gantt_loaded, what='the Gantt rows')['gantt']['keys']
        front = Foreground(app.process.pid)
        front.activate(app.process)
        front.arm()
        guarded_key(app, front, 'the first Down arrow', KEY_CODES['down'])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] != order[0], 10, 'the first real Down arrow to move the cursor')
        guarded_key(app, front, '/ to the filter', KEY_CODES['slash'])
        guarded_wait(app, front, lambda s: editing(s) and editor_holds(s, ''), 10, '/ to give the filter\'s real editor the keyboard')
        ax_focus(app, front, True, 'the filter as the AX focused element after /')
        guarded_key(app, front, 'c in the filter', KEY_CODES['c'])
        typed = guarded_wait(app, front, lambda s: editing(s) and current(s).get('length', 0) > 0, 10, 'c in the filter\'s real editor')
        if current(typed).get('has_marked_text') is not True:
            uninterrupted(front, 'the input source is judged')
            not_assessed(case, f'the selected input source made no marked text of a physical c (the real editor held {current(typed)}): a plain keyboard, so no '
                               'composition could be established and Escape during one is NOT ASSESSED; the input source was not changed and no marked text was made')
            assert app.quit() == 0
            return None
        marked = current(typed)
        before = filter_editor(typed)['sequence']

        # (a) An Escape during the composition is the input method's: the field keeps the keyboard, none of it marked.
        guarded_key(app, front, 'Escape during the composition', KEY_CODES['escape'])
        guarded_wait(app, front, lambda s: after(s, before) and current(s).get('has_marked_text') is False, 10, 'the input method to end the composition on Escape')
        ax_focus(app, front, True, 'the filter as the AX focused element after Escape during the composition')
        cancelled = guarded_wait(app, front, lambda s: editing(s) and current(s).get('has_marked_text') is False, 10,
                                 'the filter and its real editor to keep the keyboard after Escape during the composition')
        assert (cancelled['gantt']['focus_target'] or '') == FILTER, f'Escape during the composition named {cancelled["gantt"]["focus_target"]!r}: the app took the input method\'s key'
        left_text = current(cancelled)['string']

        # (b) A composition committed by Return is committed text, in the real editor and in the filter.
        guarded_key(app, front, 'c to compose again', KEY_CODES['c'])
        composing = guarded_wait(app, front, lambda s: editing(s) and current(s).get('has_marked_text') is True, 10, 'c to make marked text again')
        before = filter_editor(composing)['sequence']
        guarded_key(app, front, 'Return to commit the composition', KEY_CODES['return'])
        committed = guarded_wait(app, front, lambda s: after(s, before) and editing(s) and current(s).get('has_marked_text') is False and current(s).get('length', 0) > 0
                                 and s['gantt']['filter']['text'] == current(s)['string'], 10, 'Return to commit the composition in the real editor and the filter')
        commit_text = current(committed)['string']
        ax_focus(app, front, True, 'the filter as the AX focused element after the commit')

        # (c) Cleared, then an ordinary Escape with no marked text: the one native handoff, then one Down and one Up.
        guarded_key(app, front, 'Command-A in the filter', KEY_CODES['a'], COMMAND_FLAG)
        guarded_wait(app, front, lambda s: editor_holds(s, commit_text) and current(s)['selected_range'] == [0, current(s)['length']], 10, 'Command-A to select the committed text')
        guarded_key(app, front, 'Delete to clear the filter', KEY_CODES['delete'])
        cleared = guarded_wait(app, front, lambda s: editing(s) and editor_holds(s, '') and gantt_loaded(s) and s['gantt']['rows'] == s['gantt']['total_rows'], 10,
                               'Delete to clear the filter in its real editor, every row shown')
        cursor = cleared['gantt']['cursor_key']
        row = f'gantt.row.{cursor}'
        guarded_key(app, front, 'the ordinary Escape to leave the filter', KEY_CODES['escape'])
        guarded_wait(app, front, lambda s: s['gantt']['filter']['editing'] is False and not current(s).get('first_responder'), 10,
                     'the filter and its real editor to release the keyboard after the ordinary Escape')
        back, facts = return_focus(app, front, 'the ordinary Escape from the filter', row)
        keys = back['gantt']['keys']
        at = keys.index(cursor)
        first, second = ('down', 'up') if at + 1 < len(keys) else ('up', 'down')
        neighbour = keys[at + 1] if first == 'down' else keys[at - 1]
        ready = {'cursor': cursor, 'focus_target': back['gantt']['focus_target'], 'focus_reported': back['gantt']['focus_reported'], 'filter': back['gantt']['filter'],
                 'editor': filter_editor(back).get('current'), 'ax_focused': facts['focused'][:2], 'ax_region': facts['receiver']['region']}
        guarded_key(app, front, f'{first} after the ordinary Escape', KEY_CODES[first])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == neighbour, 10, f'a real {first} arrow to move the cursor off {cursor} after the ordinary Escape')
        guarded_key(app, front, f'{second} after the ordinary Escape', KEY_CODES[second])
        guarded_wait(app, front, lambda s: s['gantt']['cursor_key'] == cursor, 10, f'a real {second} arrow to bring the cursor back to {cursor}')
        uninterrupted(front, 'the route is complete')
        if app.process.poll() is not None:
            raise AssertionError(f'the app exited ({app.process.returncode}) at the end of the composition route')
        assert app.quit() == 0
        return (f'the guarded composition route: a physical c made marked text in the filter\'s real editor ({marked}); one Escape was the input method\'s, '
                f'the editor kept the keyboard with no marked text ({left_text!r} left, {"cancelled" if not left_text else "unmarked by the input method"}) and the field '
                f'stayed the AX focused element; c and Return committed {commit_text!r}; cleared, an ordinary Escape gave the Gantt\'s own region the keyboard and '
                f'one real {first} and one {second} moved the cursor off {cursor} and back (ready on: {ready}); seen by the host and AX, not an input-method acceptance')
    except NotAssessed as interruption:
        not_assessed(case, interruption_or_death(app.process, interruption))
        return None
    finally:
        if front:
            front.close()
        app.close()


def latch_control(bundle, directory, first):
    """With the first owned app armed as a route: a real Down arrow is sent and moves its cursor while nothing interrupts
    it; then the second owned app is really made frontmost and the first really activated again, and after that no key is
    sent, no satisfied wait counts and the route cannot pass, each stop NOT ASSESSED while the app runs. What was observed."""
    second = App(bundle, directory, ['--database', str(gantt_store(directory, 'guard-second')), '--page', 'gantt'])
    front = other = again = None
    try:
        second.wait(gantt_loaded, what='the Gantt rows of the second guard app')
        front = Foreground(first.process.pid)
        front.activate(first.process)
        front.arm()
        before = first.state()['gantt']['cursor_key']
        guarded_key(first, front, 'the Down arrow of the undisturbed guard route', KEY_CODES['down'])
        guarded_wait(first, front, lambda s: s['gantt']['cursor_key'] != before, 10, 'a real Down arrow to move the cursor of the undisturbed guard route')
        mark = len(front.events)
        other = Foreground(second.process.pid)
        other.activate(second.process)
        if not front.until(lambda: 'AXApplicationDeactivated' in front.events[mark:], 5):
            raise NotAssessed(f'the second app was made frontmost, but the first app\'s deactivation was not observed within 5 s (observed: {front.events[mark:]})')
        # The first app activated again by an observer of its own, not armed: the armed route only observes it.
        again = Foreground(first.process.pid)
        again.activate(first.process)
        if not front.until(lambda: 'AXApplicationActivated' in front.events[front.events.index('AXApplicationDeactivated', mark):], 5):
            raise NotAssessed(f'the first app was made frontmost again, but its activation was not observed within 5 s (observed: {front.events[mark:]})')
        regained = front.current_problem()
        if regained:
            raise NotAssessed(f'the first app was activated again but does not hold the keyboard: {regained}')
        sequence = front.events[mark:]
        stops = []
        for what, attempt in (('a key', lambda: guarded_key(first, front, 'a key after the observed deactivation', KEY_CODES['down'])),
                              ('a satisfied wait', lambda: guarded_wait(first, front, lambda s: True, 5, 'a state the app already shows')),
                              ('the route\'s pass', lambda: uninterrupted(front, 'the route is complete'))):
            try:
                attempt()
            except NotAssessed as stop:
                stops.append(interruption_or_death(first.process, stop))
            else:
                raise AssertionError(f'after a real deactivation and activation again ({sequence}), with the app frontmost and its window key, {what} was allowed')
        return (f'a real Down arrow moved the cursor of the armed, undisturbed route; after the observed {sequence} and with the app frontmost and its '
                f'window key again, a key, a satisfied wait and the route\'s pass were each refused, NOT ASSESSED while the app ran ({stops[0]})')
    finally:
        for observer in (again, other, front):
            if observer:
                observer.close()
        second.close()


def dead_app_control(process):
    """Accessibility and activation requests for an owned app that has exited: what stops them is its exit, a failure
    keeping its status and the error, never NOT ASSESSED. Which request found it."""
    reader = None
    try:
        try:
            reader = Foreground(process.pid)
            reader.activate(process)
        except NotAssessed as interruption:
            try:
                interruption_or_death(process, interruption)
            except AssertionError as failure:
                return f'the NOT ASSESSED of the stopped request became the failure: {failure}'
            raise AssertionError(f'an app that had exited ({process.returncode}) was reported as an interruption: {interruption}')
        except AssertionError as failure:
            return f'the activation request itself failed: {failure}'
    finally:
        if reader:
            reader.close()
    raise AssertionError(f'an app that had exited ({process.returncode}) was made frontmost')


def keyboard_guard_controls(bundle, directory):
    """The keyboard route's guards, against the suite's own apps on disposable stores: an observed deactivation survives
    a later activation (latch_control), and an exited app is a failure inside the accessibility and activation branches
    (dead_app_control). The latch needs the Accessibility permission; without it, it is NOT ASSESSED and the exit still runs."""
    case = 'keyboard_guard_controls (interruption latch)'
    first = App(bundle, directory, ['--database', str(gantt_store(directory, 'guard-first')), '--page', 'gantt'])
    latch = None
    try:
        first.wait(gantt_loaded, what='the Gantt rows of the first guard app')
        if not accessibility_trusted():
            not_assessed(case, 'this process has no Accessibility permission, so no key can be sent; the latch was not exercised')
        else:
            try:
                latch = latch_control(bundle, directory, first)
            except NotAssessed as interruption:
                not_assessed(case, interruption_or_death(first.process, interruption))
        assert first.quit() == 0
        death = dead_app_control(first.process)
    finally:
        first.close()
    return (f'route guards: {latch if latch else "the interruption latch was NOT ASSESSED (reported above)"}; and after the owned app exited, {death}')


def main(bundle):
    if sys.platform != 'darwin':
        print(f'SKIPPED: the observer is a macOS app and this is {sys.platform}; nothing was launched and nothing is claimed.')
        return []
    results = []
    with tempfile.TemporaryDirectory() as scratch:
        directory = Path(scratch)
        packaged = directory / 'DPMObserver.app'
        shutil.copytree(bundle, packaged, symlinks=True)
        for scenario in (review_and_verify, reachable_work, latency_and_burst, selected_run_window, preview_and_refusals, stale_and_source_change,
                         gantt_page, gantt_window_layout, gantt_real_keys, detail_accessibility, detail_scroll_route, detail_keyboard_route,
                         filter_exit_route, filter_composition_route, keyboard_guard_controls, gantt_instruments, gantt_scroll_guards, gantt_rendered):
            result = scenario(packaged, directory)
            # A case that was skipped or not assessed says so itself, loudly, and returns nothing: it is never a pass.
            if result is not None:
                results.append(result)
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
    # The exit status says only that nothing failed; what was not assessed is said here, and this run is then not a full pass.
    for line in NOT_ASSESSED:
        print(f'NOT ASSESSED: {line}')
    if NOT_ASSESSED:
        print(f'NOT A FULL PASS: {len(NOT_ASSESSED)} case(s) above were not assessed; no PASS line covers them.')
