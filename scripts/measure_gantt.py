#!/usr/bin/env python3
"""Measure the packaged DPM Observer's Gantt with the instruments of the accepted design record
(FEAT-05 correction 2, section c.5), and generate the datasets the Gantt suites use.

    python3 scripts/measure_gantt.py --out PATH            the measurements and the controls
    python3 scripts/measure_gantt.py --out PATH --probe    plus the 5000-task scale probe
    python3 scripts/measure_gantt.py --self-test           the analyzer and the evidence capture, on frozen logs

The app is launched as shipped (a copy of the built bundle) against disposable stores written through
the real CLI, with `--measure-log`, and what it measured is read back from its append-only JSON Lines
log: every timestamp is the app's own `CLOCK_UPTIME_RAW`, and a latency or a frame exists only for a
generation that an app-owned draw pass stamped as drawn. A display-link `tick`, a model assignment
(`state_set`) and a state-file value complete nothing. Before any number is kept the negative control
runs: with a surface's draws withheld (`--measure-freeze`) while the callbacks and the model carry on,
no latency, frame or drawn event may exist, and the same analyzer that judges a measurement must call
that log INVALID and give no number.

ONE analyzer (the `analyze_*` functions) judges every run, a measurement or a control or a replayed log:
all expected generations, timeouts and late draws, completeness, content facts and the integrity of the
log, BEFORE any statistic is eligible. An INVALID run keeps its raw observations and contributes no number
to an aggregate; a required run that is INVALID fails the script; when a control fails no aggregate is
published as qualified. Threshold misses are reported apart (`gates`), never as an invalid instrument.

Scroll method (FEAT-20 measurement configuration, a bounded clarification the root approved; the sealed
FEAT-05 files are unchanged). The content is finite, so a request of 40 points per number cannot be an
offset forever. A request carries its LOGICAL DISTANCE (40 * its number in the window); the content wraps
over its scroll range (extent less viewport), and the offset it must be drawn at is the EXPECTED EFFECTIVE
OFFSET, which the driver works out apart from the draw. The draw stamps the GEOMETRY DRAWN OFFSET, the one
the content was actually placed at. Per frame the log records logical_distance, expected_effective_offset,
geometry_drawn_offset, axis, window_id, extent and viewport. A frame counts only for a unique requested
generation (window, axis, number), only when the offset actually moved, and only when expected equals drawn;
a content that does not scroll (no range) is not scroll FPS. Windows of 5.0 s, the discarded half second, the
warm-up and the repetitions, the 30 frames/s target, the meaning of a stamp (draw completion, not display
presentation) and the INVALID rules are as the accepted record defines them.

The script enforces no threshold. It writes one document with `targets` (the live contract and the
design proposals) and `measured` (raw samples, aggregates) in separate objects, and the reviewer judges.
It exits nonzero, naming what, when an instrument is missing or a run was invalid or did not complete.
Every launch (warm-ups and failures included) keeps its exact argv, raw log, state, stdout, stderr, outcome
and their sha256 in a directory next to the report, named after it; only the disposable stores are temporary.

Skipped loudly, not passed, off macOS.
"""
import argparse
import copy
import hashlib
import json
import math
import os
import platform
import select
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from smoke_agent import ROOT, run_cli
from smoke_performance import generated

FIXTURE = ROOT / 'tests/support/execution-plan.json'
WORKER = 'agent:worker'
REPETITIONS = 7
FIRST_DRAW_TIMEOUT = 10.0
GENERATION_TIMEOUT = 5.0
# Points of logical travel per scroll request, and the zoom the app starts at (GanttLayout.defaultZoom), as the app defines them.
FRAME_STEP = 40.0
DEFAULT_ZOOM = 3
ZOOM_LEVELS = 8
# What the app's `viewops` script does, round by round, and how often: the planned count of each operation kind.
# Five blocks of ten calls: two of each kind per block, so ten per kind in all. The first filter of a block turns the
# critical filter on and the second clears it, with only zoom and pan between them (never a collapse or an expand).
VIEWOPS_ROUNDS = 5
VIEWOPS_SCRIPT = ['collapse_all', 'expand_all', 'filter', 'zoom', 'pan', 'filter', 'collapse_all', 'expand_all', 'zoom', 'pan']
PLANNED_VIEW_OPS = {'collapse_all': 10, 'expand_all': 10, 'zoom': 10, 'pan': 10, 'filter': 10}
FINGERPRINT = ('zoom', 'pan_x', 'pan_y', 'filter', 'collapsed')


# --------------------------------------------------------------------------------------------------
# Datasets (record c.2): the generator of scripts/smoke_performance.py, with the overlays it names.
# --------------------------------------------------------------------------------------------------

def task_id(i):
    return str(uuid.UUID(int=(77 << 96) + i + 1))


def package_id(n):
    return str(uuid.UUID(int=(80 << 96) + n))


def package_count(count):
    return math.ceil(count / 10)


def edge(identity, predecessor, successor, policy='Hard', kind='FinishStart', lag=0.0):
    return dict(id=str(identity), predecessor=predecessor, successor=successor, kind=kind, lag_hours=lag, policy=policy)


def add_hierarchy(plan, count):
    """DS-H: P = ceil(N/10) work packages, task i under package i // 10 + 1."""
    template = next(w for w in plan['work_items'].values() if w['kind'] == 'Task')
    for n in range(1, package_count(count) + 1):
        package = copy.deepcopy(template)
        package.update(id=package_id(n), key=f'PERF-P{n}', kind='WorkPackage', parent=None, title=f'Package {n}', order=[count + n])
        package['contract'] = dict(objective=f'The tasks grouped under package {n}.', acceptance=[dict(text='Every child is verified.')],
                                   capabilities=[], requirement_ids=[], assets=[])
        package['schedule'] = dict(priority='P2', estimate=None)
        plan['work_items'][package['id']] = package
    for i in range(count):
        plan['work_items'][task_id(i)]['parent'] = package_id(i // 10 + 1)
    return plan


def add_dense(plan, count):
    """DS-D: layers of three; every task from the fourth on has two predecessors in the layer before,
    the second of them Soft when i mod 4 == 0. Acyclic: every edge goes from a lower index to a higher."""
    edges = []
    for i in range(3, count):
        layer, r = divmod(i, 3)
        base = 3 * (layer - 1)
        edges.append(edge(uuid.UUID(int=(78 << 96) + i), task_id(base + r), task_id(i)))
        edges.append(edge(uuid.UUID(int=(79 << 96) + i), task_id(base + (r + 1) % 3), task_id(i), 'Soft' if i % 4 == 0 else 'Hard'))
    plan['dependencies'] = edges
    return plan


def add_long_titles(plan, count):
    """ST-L: the title of every task with i mod 10 == 0 is a 500-character string."""
    for i in range(0, count, 10):
        plan['work_items'][task_id(i)]['title'] = (f'PERF-{i + 1} a very long title that must stay readable and complete. ' * 12)[:500]
    return plan


def dataset(count, shape, hierarchy=True, long_titles=False):
    """`shape`: `branch` (DS-B), `chain` (DS-C) or `dense` (DS-D). Hierarchy (DS-H) is added by default."""
    plan = generated(count, 'flat' if shape == 'dense' else shape)
    if shape == 'dense':
        add_dense(plan, count)
    if hierarchy:
        add_hierarchy(plan, count)
    if long_titles:
        add_long_titles(plan, count)
    return plan


def describe(plan):
    """Counts read from the generated plan itself, for the record and for the checks against c.2."""
    items = plan['work_items'].values()
    tasks = [w for w in items if w['kind'] == 'Task']
    packages = [w for w in items if w['kind'] == 'WorkPackage']
    soft = sum(1 for e in plan['dependencies'] if e.get('policy') == 'Soft')
    return dict(tasks=len(tasks), packages=len(packages), rows_expanded=len(tasks) + len(packages), rows_collapsed=len(packages) or len(tasks),
                edges=len(plan['dependencies']), soft_edges=soft, hard_edges=len(plan['dependencies']) - soft)


def fx_gantt(calendars=False):
    """FX-J1 on the test fixture: a work package holding three tasks (so a nested task exists), the
    fixture's milestone and its finish-to-start edges, and one each of start-to-start (lag, Soft),
    finish-to-finish (a lead) and start-to-finish (lag). One task has a 500-character title."""
    plan = json.loads(FIXTURE.read_text())
    by_key = {w['key']: w for w in plan['work_items'].values()}
    package = copy.deepcopy(by_key['TEST-M1'])
    package.update(id='00000007-0000-4000-8000-0000000000a1', key='TEST-PKG', kind='WorkPackage', title='Work package', order=[50])
    plan['work_items'][package['id']] = package
    for key in ('TEST-A', 'TEST-B', 'TEST-C'):
        by_key[key]['parent'] = package['id']
    by_key['TEST-F']['title'] = ('Contract F with a long title that must be read in full, never cut for width. ' * 8)[:500]
    ids = {key: work['id'] for key, work in by_key.items()}
    for n, (pred, succ, kind, lag, policy) in enumerate([('TEST-A', 'TEST-C', 'StartStart', 2.0, 'Soft'), ('TEST-A', 'TEST-E', 'FinishFinish', -1.0, 'Hard'),
                                                          ('TEST-B', 'TEST-D', 'StartFinish', 3.0, 'Hard')], 1):
        plan['dependencies'].append(edge(uuid.UUID(int=(90 << 96) + n), ids[pred], ids[succ], policy, kind, lag))
    if calendars:
        plan['calendars'] = dict(time_zone='Europe/Berlin')
    return plan


def write_plan(path, plan):
    Path(path).write_text(json.dumps(plan))
    return Path(path)


# --------------------------------------------------------------------------------------------------
# The app, launched as shipped, its measurement log and the evidence every launch keeps.
# --------------------------------------------------------------------------------------------------

class RunFailed(Exception):
    """A run did not complete, or an instrument is missing: the reason names which."""


def uptime_ns():
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def read_log(path):
    """Every event of a log, in order; a torn last line is dropped."""
    events = []
    try:
        for line in Path(path).read_text().splitlines():
            try:
                events.append(json.loads(line))
            except ValueError:
                continue
    except OSError:
        pass
    return events


class Capture:
    """Where every launch keeps its raw evidence, and which launches there were. With no root the launch
    directory is made next to the disposable stores (the smoke suites); the measurement sets a root."""

    def __init__(self):
        self.root = None
        self.launches = []


CAPTURE = Capture()


class Launched:
    """One launch of the packaged app with a measurement log, watched by events and never by sleeping.

    Everything it produces is kept in a directory of its own, `<name>-<measure id>`: `argv.json` (the exact command),
    `log.jsonl`, `state.json` (the app's last state), `stdout.txt`, `stderr.txt`, `outcome.json` (how the process
    ended and any failure) and `sha256.json` (the hash of each of them)."""

    def __init__(self, bundle, directory, name, arguments, freeze=()):
        self.name = name
        self.measure_id = str(uuid.uuid4())
        self.dir = Path(CAPTURE.root or directory) / f'{name}-{self.measure_id}'
        self.dir.mkdir(parents=True)
        self.log_path = self.dir / 'log.jsonl'
        self.state_path = self.dir / 'state.json'
        self.command = [str(Path(bundle) / 'Contents/MacOS/dpm-observer'), *arguments, '--state-file', str(self.state_path),
                        '--measure-log', str(self.log_path), '--measure-id', self.measure_id]
        if freeze:
            self.command += ['--measure-freeze', ','.join(freeze)]
        self.events = []
        self.by_name = {}
        self.failure = None
        self.finalized = False
        self._offset = 0
        self._buffer = b''
        self._log_fd = None
        (self.dir / 'argv.json').write_text(json.dumps(dict(argv=self.command, cwd='/', measure_id=self.measure_id, started_at=datetime.now(timezone.utc).isoformat()), indent=2) + '\n')
        self._stdout = open(self.dir / 'stdout.txt', 'wb')
        self._stderr = open(self.dir / 'stderr.txt', 'wb')
        self._queue = select.kqueue()
        self._watched = os.open(self.dir, os.O_RDONLY)
        # The directory is watched before the app starts: its log appearing and its state being replaced are
        # writes to it, and the process ending is the other event.
        self._queue.control([select.kevent(self._watched, select.KQ_FILTER_VNODE, select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                                           select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND | select.KQ_NOTE_RENAME | select.KQ_NOTE_LINK | select.KQ_NOTE_DELETE)], 0, 0)
        # Read immediately before the spawn, on the clock the app stamps with (record c.1).
        self.t0 = uptime_ns()
        self.process = subprocess.Popen(self.command, cwd='/', stdout=self._stdout, stderr=self._stderr)
        try:
            self._queue.control([select.kevent(self.process.pid, select.KQ_FILTER_PROC, select.KQ_EV_ADD | select.KQ_EV_CLEAR, select.KQ_NOTE_EXIT)], 0, 0)
        except OSError:
            pass  # it has ended already: poll() says so
        CAPTURE.launches.append(self)

    # -- events -----------------------------------------------------------------------------------

    def _watch_log(self):
        """Watch the log's own writes once it exists. True when it was just registered, so the caller reads it again."""
        if self._log_fd is not None or not self.log_path.exists():
            return False
        self._log_fd = os.open(self.log_path, os.O_RDONLY)
        self._queue.control([select.kevent(self._log_fd, select.KQ_FILTER_VNODE, select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                                           select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND)], 0, 0)
        return True

    def wait_activity(self, timeout):
        """Block until the log grows, the state is replaced or the process ends, or the timeout is out."""
        if timeout <= 0 or self._watch_log():
            return
        self._queue.control(None, 16, timeout)

    def pump(self):
        try:
            with open(self.log_path, 'rb') as handle:
                handle.seek(self._offset)
                chunk = handle.read()
        except OSError:
            return
        if not chunk:
            return
        self._offset += len(chunk)
        lines = (self._buffer + chunk).split(b'\n')
        self._buffer = lines.pop()
        for line in lines:
            if not line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                continue
            self.events.append(event)
            self.by_name.setdefault(event.get('event'), []).append(event)

    def state(self):
        try:
            return json.loads(self.state_path.read_text())
        except (OSError, ValueError):
            return None

    def wait_event(self, name, timeout, gen=None):
        deadline = time.monotonic() + timeout
        while True:
            self.pump()
            for event in self.by_name.get(name, []):
                if gen is None or event.get('gen') == gen:
                    return event
            if self.process.poll() is not None:
                self.pump()
                return next((e for e in self.by_name.get(name, []) if gen is None or e.get('gen') == gen), None)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return None
            self.wait_activity(remaining)

    def wait_state(self, predicate, timeout, what):
        deadline = time.monotonic() + timeout
        while True:
            last = self.state()
            if last is not None and predicate(last):
                return last
            if self.process.poll() is not None:
                raise RunFailed(f'the app exited ({self.process.returncode}) while waiting for {what}')
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RunFailed(f'timed out waiting for {what}; last state: {json.dumps(last, sort_keys=True)[:600]}')
            self.wait_activity(remaining)

    # -- ending ----------------------------------------------------------------------------------

    def quit(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=30)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                self.pump()
                self.failure = self.failure or 'the app did not quit within 30 seconds'
                raise RunFailed('the app did not quit within 30 seconds')
        self.pump()
        return self.process.returncode

    def close(self):
        """End the app if it still runs, keep what it produced and release what watched it. Safe to call again."""
        try:
            self.quit()
        except RunFailed:
            pass
        self.finalize()

    def finalize(self, failure=None):
        """Write the outcome and the hashes of everything this launch kept. The process end is recorded once; called again
        it rewrites nothing unless a failure annotation is new, and then the hashes follow the new outcome."""
        annotated = bool(failure) and failure != self.failure
        if failure:
            self.failure = failure
        if self.finalized and not annotated and (self.dir / 'sha256.json').exists():
            return json.loads((self.dir / 'sha256.json').read_text())
        if not self.finalized:
            self.finalized = True
            for handle in (self._stdout, self._stderr):
                handle.close()
            if self._log_fd is not None:
                os.close(self._log_fd)
                self._log_fd = None
            os.close(self._watched)
            self._queue.close()
        code = self.process.returncode
        if getattr(self, 'ended_at', None) is None:
            self.ended_at = datetime.now(timezone.utc).isoformat()
        outcome = dict(returncode=code, signal=-code if code is not None and code < 0 else None, ended_at=self.ended_at,
                       events=len(self.events), failure=self.failure)
        (self.dir / 'outcome.json').write_text(json.dumps(outcome, indent=2) + '\n')
        hashes = {path.name: sha256(path) for path in sorted(self.dir.iterdir()) if path.is_file() and path.name != 'sha256.json'}
        (self.dir / 'sha256.json').write_text(json.dumps(hashes, indent=2, sort_keys=True) + '\n')
        return hashes

    def display_facts(self):
        """What the app itself logged about its display link, kept with the launch: the link, each recorded state
        (screen, window visibility and occlusion, app activity, paused) and the ticks seen. Observed, never inferred."""
        names = by_name(self.events)
        return dict(display_link=[detail_of(e) for e in names.get('display_link', []) + names.get('no_display', [])],
                    display_state=[dict(detail_of(e), t_ns=e.get('t_ns')) for e in names.get('display_state', [])], ticks=len(names.get('tick', [])),
                    no_ticks=not names.get('tick'))

    def evidence(self):
        root = CAPTURE.root
        return dict(directory=str(self.dir.relative_to(root.parent)) if root else str(self.dir), measure_id=self.measure_id, name=self.name,
                    returncode=self.process.returncode, display=self.display_facts(), sha256=json.loads((self.dir / 'sha256.json').read_text()) if (self.dir / 'sha256.json').exists() else None)


def loaded(state):
    return state['connection'] == 'connected' and state['freshness']['current'] and state['gantt']['loaded']


# --------------------------------------------------------------------------------------------------
# THE ANALYZER. One path judges every run, a measurement, a control or a replayed log, before any number
# is eligible. Every `analyze_*` returns `invalid` (the reasons), `raw` (what was observed, kept always)
# and `values` (the numbers a qualified run yields; None for an INVALID run).
# --------------------------------------------------------------------------------------------------

def by_name(events):
    found = {}
    for event in events:
        found.setdefault(event.get('event'), []).append(event)
    return found


def detail_of(event):
    return event.get('detail') or {}


def as_float(value):
    if isinstance(value, bool):
        return None
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def close_to(a, b, tolerance=1e-6):
    return a is not None and b is not None and abs(a - b) <= tolerance


def ms(a, b):
    return (b - a) / 1e6


def stats(values):
    if not values:
        return dict(n=0, min=None, median=None, max=None)
    ordered = sorted(values)
    return dict(n=len(values), min=ordered[0], median=statistics.median(ordered), max=ordered[-1], p99=ordered[min(len(ordered) - 1, math.ceil(0.99 * len(ordered)) - 1)])


def integrity(events, run_id=None, need_ticks=True):
    """The reasons the log itself, or the run that wrote it, is not to be trusted (c.5.0): a log that is not
    one run's complete record, a dropped event, no display ticks, a generation that timed out (which stays
    invalid even if a draw eventually appeared) and an error of the driver."""
    reasons = set()
    names = by_name(events)
    opened, closed = names.get('log_opened', []), names.get('log_closed', [])
    if len(opened) != 1 or (events and events[0].get('event') != 'log_opened'):
        reasons.add('LOG_INTEGRITY')
    if not closed:
        reasons.add('LOG_NOT_CLOSED')
    elif len(closed) > 1 or events[-1].get('event') != 'log_closed':
        reasons.add('LOG_INTEGRITY')
    else:
        if closed[0].get('dropped', 0) > 0:
            reasons.add('EVENTS_DROPPED')
    if opened:
        identity, process = opened[0].get('measure_id'), opened[0].get('pid')
        if run_id is not None and identity != run_id:
            reasons.add('LOG_INTEGRITY')
        if any(e.get('measure_id') not in (None, identity) or e.get('pid') not in (None, process) for e in events):
            reasons.add('LOG_INTEGRITY')
    if need_ticks and not names.get('tick'):
        reasons.add('NO_TICKS')
    if names.get('generation_timeout'):
        reasons.add('GENERATION_TIMEOUT')
    if names.get('driver_error'):
        reasons.add('DRIVER_ERROR')
    return reasons


def late(call_ns, done_ns, limit=GENERATION_TIMEOUT):
    return ms(call_ns, done_ns) > limit * 1000


def outcome_of(status, reasons, raw, values, **more):
    invalid = sorted(reasons)
    return dict(status=status, invalid=invalid, raw=raw, values=None if invalid else values, **more)


def analyze_bf1(events, expected_rows, spawn_ns, run_id=None):
    """BF1: the first draw, from the spawn and from the process's own entry."""
    reasons = integrity(events, run_id)
    names = by_name(events)
    first = (names.get('first_draw') or [None])[0]
    entry = (names.get('process_entry') or [None])[0]
    if first is None:
        reasons.add('FIRST_DRAW_NOT_REACHED')
        return outcome_of('incomplete', reasons, dict(expected_rows=expected_rows), None)
    detail = detail_of(first)
    if detail.get('model_rows') != expected_rows:
        reasons.add('CONTENT_MISMATCH')
    ordered = entry is not None and spawn_ns <= entry['t_ns'] and entry['t_ns'] - spawn_ns < 10e9
    if not ordered:
        reasons.add('CLOCK_ORDER')
    if ms(spawn_ns, first['t_ns']) > FIRST_DRAW_TIMEOUT * 1000:
        reasons.add('LATE_GENERATION')
    raw = dict(expected_rows=expected_rows, model_rows=detail.get('model_rows'), viewport_rows_drawn=detail.get('viewport_rows_drawn'),
               first_draw_ms_from_spawn=ms(spawn_ns, first['t_ns']), first_draw_ms_since_process_entry=ms(entry['t_ns'], first['t_ns']) if entry else None,
               clock_ordering_confirmed=ordered, t0_ns=spawn_ns, process_entry_ns=entry['t_ns'] if entry else None, first_draw_ns=first['t_ns'])
    return outcome_of('drawn', reasons, raw, dict(first_draw_ms_from_spawn=raw['first_draw_ms_from_spawn'], first_draw_ms_since_process_entry=raw['first_draw_ms_since_process_entry']))


def analyze_bf2(events, keys, run_id=None):
    """BF2: selection to Detail. The planned selections must all have been called, each in turn on its own
    task, and each drawn with that task, not loading, and as many sections as the model assigned."""
    reasons = integrity(events, run_id)
    names = by_name(events)
    calls = names.get('select_call', [])
    if len(calls) < len(keys):
        reasons.add('MISSING_REQUIRED_OPERATION')
    elif len(calls) > len(keys):
        reasons.add('UNPLANNED_OPERATION')
    drawn, modelled = {}, {}
    for event in names.get('detail_drawn', []):
        if event.get('gen') in drawn:
            reasons.add('DUPLICATE_GENERATION')
        drawn.setdefault(event.get('gen'), event)
    for event in names.get('state_set', []):
        if detail_of(event).get('kind') == 'selection':
            modelled.setdefault(event.get('gen'), event)
    rows = []
    for index, call in enumerate(calls):
        done = drawn.get(call.get('gen'))
        row = dict(index=index, kept=index > 0, subject=detail_of(call).get('subject'), gen=call.get('gen'), latency_ms=None, status='incomplete')
        if index < len(keys) and row['subject'] != keys[index]:
            reasons.add('CONTENT_MISMATCH')
        if done is None:
            row['reason'] = 'INCOMPLETE_GENERATION'
            reasons.add('INCOMPLETE_GENERATION')
        else:
            row.update(status='drawn', latency_ms=ms(call['t_ns'], done['t_ns']), sections_drawn=detail_of(done).get('sections'))
            if done['t_ns'] < call['t_ns']:
                reasons.add('ORDER_VIOLATION')
            if late(call['t_ns'], done['t_ns']):
                reasons.add('LATE_GENERATION')
            if detail_of(done).get('subject') != row['subject'] or detail_of(done).get('loading') != 'false':
                reasons.add('CONTENT_MISMATCH')
            model = modelled.get(call.get('gen'))
            if model is None:
                reasons.add('MISSING_FACT')
            else:
                row['sections_model'] = detail_of(model).get('sections')
                row['model_to_draw_ms'] = ms(model['t_ns'], done['t_ns'])
                if row['sections_drawn'] != row['sections_model']:
                    reasons.add('CONTENT_MISMATCH')
        rows.append(row)
    values = dict(kept_latency_ms=[r['latency_ms'] for r in rows if r['kept'] and r['latency_ms'] is not None])
    return outcome_of('analyzed', reasons, dict(rows=rows), values, rows=rows)


def planned_views(record):
    """What each operation of the `viewops` script must leave, worked out from the plan and the script and not
    from the app: the zoom level, the filter, the vertical pan, the collapsed count and, where the plan says it, the rows."""
    expected, zoom, flip = [], DEFAULT_ZOOM, 0
    collapsed = 0
    for kind in VIEWOPS_SCRIPT * VIEWOPS_ROUNDS:
        facts = {}
        if kind == 'collapse_all':
            collapsed = record['packages']
            facts['rows'] = str(record['rows_collapsed'])
        elif kind == 'expand_all':
            collapsed = 0
            facts['rows'] = str(record['rows_expanded'])
        elif kind == 'zoom':
            zoom = max(0, min(ZOOM_LEVELS - 1, zoom + (1 if flip % 2 == 0 else -1)))
            flip += 1
        elif kind == 'pan':
            facts['pan_x'] = None
        facts.update(zoom=str(zoom), collapsed=str(collapsed), pan_y='0.000')
        expected.append((kind, facts))
    return expected


def analyze_bf4(events, record, run_id=None, planned=None, script=True):
    """BF4: view operations. Every planned operation, by kind and in number, must have been called on entry,
    expected after it was applied, and drawn with the rows and the view state (zoom, pan, filter, collapse) it
    expected; and the plan and the script must agree with both."""
    planned = PLANNED_VIEW_OPS if planned is None else planned
    reasons = integrity(events, run_id)
    names = by_name(events)
    calls = names.get('view_op_call', [])
    counts = {}
    for call in calls:
        counts[detail_of(call).get('op')] = counts.get(detail_of(call).get('op'), 0) + 1
    for kind, number in planned.items():
        if counts.get(kind, 0) < number:
            reasons.add('MISSING_REQUIRED_OPERATION')
        elif counts.get(kind, 0) > number:
            reasons.add('UNPLANNED_OPERATION')
    if any(kind not in planned for kind in counts):
        reasons.add('UNPLANNED_OPERATION')
    states, drawn = {}, {}
    for event in names.get('state_set', []):
        if detail_of(event).get('kind') == 'view_op':
            states.setdefault(event.get('gen'), event)
    for event in names.get('view_op_drawn', []):
        if event.get('gen') in drawn:
            reasons.add('DUPLICATE_GENERATION')
        drawn.setdefault(event.get('gen'), event)
    if len({c.get('gen') for c in calls}) != len(calls):
        reasons.add('DUPLICATE_GENERATION')
    if names.get('view_op_mismatch'):
        reasons.add('CONTENT_MISMATCH')
    expectation = planned_views(record) if script and len(calls) == len(VIEWOPS_SCRIPT) * VIEWOPS_ROUNDS and counts == planned else None
    rows, seen = [], {}
    for index, call in enumerate(calls):
        kind = detail_of(call).get('op')
        seen[kind] = seen.get(kind, 0) + 1
        state, done = states.get(call.get('gen')), drawn.get(call.get('gen'))
        row = dict(op=kind, ordinal=seen[kind], kept=seen[kind] > 1, gen=call.get('gen'), latency_ms=None, status='incomplete')
        if state is None:
            reasons.add('MISSING_FACT')
        else:
            expected = detail_of(state)
            row['expected_rows'] = expected.get('rows')
            if call['t_ns'] > state['t_ns']:
                reasons.add('ORDER_VIOLATION')
        if done is None:
            row['reason'] = 'INCOMPLETE_GENERATION'
            reasons.add('INCOMPLETE_GENERATION')
        else:
            shown = detail_of(done)
            row.update(status='drawn', latency_ms=ms(call['t_ns'], done['t_ns']), rows_drawn_model=shown.get('rows_drawn_model'), fingerprint={k: shown.get(k) for k in FINGERPRINT})
            if done['t_ns'] < call['t_ns'] or (state is not None and done['t_ns'] < state['t_ns']):
                reasons.add('ORDER_VIOLATION')
            if late(call['t_ns'], done['t_ns']):
                reasons.add('LATE_GENERATION')
            if state is not None:
                expected = detail_of(state)
                for name in (*FINGERPRINT, 'rows'):
                    if name not in shown or name not in expected:
                        reasons.add('MISSING_FACT')
                    elif str(shown[name]) != str(expected[name]):
                        reasons.add('CONTENT_MISMATCH')
                if shown.get('rows_drawn_model') is None or str(shown.get('rows_drawn_model')) != str(expected.get('rows')):
                    reasons.add('CONTENT_MISMATCH')
        # The plan and the script, apart from the app's own expectation.
        if kind == 'collapse_all' and str(row.get('expected_rows')) != str(record['rows_collapsed']):
            reasons.add('CONTENT_MISMATCH')
        if kind == 'expand_all' and str(row.get('expected_rows')) != str(record['rows_expanded']):
            reasons.add('CONTENT_MISMATCH')
        if expectation is not None:
            want_kind, want = expectation[index]
            if kind != want_kind:
                reasons.add('OPERATION_ORDER')
            if done is not None:
                shown = detail_of(done)
                for name, value in want.items():
                    if value is not None and str(shown.get(name)) != value:
                        reasons.add('CONTENT_MISMATCH')
                if want_kind == 'filter':
                    if str(shown.get('filter')) != ('critical only' if seen[kind] % 2 == 1 else 'no filter'):
                        reasons.add('CONTENT_MISMATCH')
                elif str(shown.get('filter')) != ('critical only' if seen.get('filter', 0) % 2 == 1 else 'no filter'):
                    # The filter is on between the first filter call of a block and the second, and nowhere else.
                    reasons.add('CONTENT_MISMATCH')
                if want_kind == 'pan' and seen[kind] % 2 == 0 and str(shown.get('pan_x')) != '0.000':
                    reasons.add('CONTENT_MISMATCH')
        rows.append(row)
    values = {kind: [r['latency_ms'] for r in rows if r['op'] == kind and r['kept'] and r['latency_ms'] is not None] for kind in planned}
    return outcome_of('analyzed', reasons, dict(rows=rows, counts=counts), values, rows=rows)


def wrap_offset(distance, span):
    """The trajectory of a scripted scroll over finite content, worked out here: the content wraps over its scroll range."""
    return distance - span * math.floor(distance / span)


def analyze_bf5(events, seconds, expected_axes=('vertical', 'horizontal'), run_id=None):
    """BF5: scroll frames. Every window of the log is judged by its own numbered requests: a frame is counted only for
    a unique requested generation (window, axis, number) of its window, whose logical distance is 40 points per number,
    whose expected effective offset the driver recorded and this analyzer works out again, whose drawn offset is the one the
    geometry placed the content at and equals it, and which actually moved the content. A content with no scroll range is
    not scroll FPS."""
    reasons = integrity(events, run_id)
    names = by_name(events)
    starts = names.get('scroll_window_start', [])
    if expected_axes is not None and [detail_of(s).get('axis') for s in starts] != list(expected_axes) * (len(starts) // len(expected_axes) if expected_axes else 0):
        reasons.add('MISSING_WINDOW')
    if not starts:
        reasons.add('MISSING_WINDOW')
    if len({detail_of(s).get('window_id') for s in starts}) != len(starts):
        reasons.add('DUPLICATE_GENERATION')
    requests, request_by_id = {}, {}
    for request in names.get('scroll_request', []):
        d = detail_of(request)
        identity = (d.get('window_id'), d.get('axis'), d.get('seq'))
        if identity in request_by_id:
            reasons.add('DUPLICATE_GENERATION')
        request_by_id.setdefault(identity, request)
        requests.setdefault(d.get('window_id'), []).append(request)
    frames_by_id = {}
    for frame in names.get('frame', []):
        d = detail_of(frame)
        identity = (d.get('window_id'), d.get('axis'), d.get('seq'))
        if identity in frames_by_id:
            reasons.add('DUPLICATE_GENERATION')
        frames_by_id.setdefault(identity, frame)
    if names.get('frame_mismatch'):
        reasons.add('CONTENT_MISMATCH')
    unavailable = {detail_of(e).get('window_id'): e for e in names.get('scroll_unavailable', [])}
    # Every frame is checked against its own request, wherever it falls in time.
    verdicts = {}
    for identity, frame in frames_by_id.items():
        d, request = detail_of(frame), request_by_id.get(identity)
        why = None
        if request is None:
            why = 'CONTENT_MISMATCH'
        else:
            r = detail_of(request)
            logical, expected, drawn = as_float(d.get('logical_distance')), as_float(d.get('expected_effective_offset')), as_float(d.get('geometry_drawn_offset'))
            extent, viewport = as_float(d.get('extent')), as_float(d.get('viewport'))
            if None in (logical, expected, drawn, extent, viewport) or d.get('seq') is None:
                why = 'MISSING_FACT'
            else:
                span = extent - viewport
                independent = wrap_offset(FRAME_STEP * d['seq'], span) if span > 0 else None
                if not (close_to(logical, FRAME_STEP * d['seq']) and close_to(logical, as_float(r.get('logical_distance'))) and close_to(expected, as_float(r.get('expected_effective_offset')))
                        and close_to(extent, as_float(r.get('extent'))) and close_to(viewport, as_float(r.get('viewport')))
                        and close_to(expected, independent, 1e-3) and close_to(drawn, expected, 1e-3)):
                    why = 'CONTENT_MISMATCH'
            if why is None and frame['t_ns'] < request['t_ns']:
                why = 'ORDER_VIOLATION'
        verdicts[identity] = why
        if why:
            reasons.add(why)
    windows, table = [], []
    for start in starts:
        axis, window_id = detail_of(start).get('axis'), detail_of(start).get('window_id')
        mine = sorted(requests.get(window_id, []), key=lambda e: detail_of(e).get('seq'))
        window = dict(axis=axis, window_id=window_id, applicable=True, requested=0, drawn=0, dropped=0, frames_per_second=None, invalid=[], intervals_ms=[], wrapped_frames=0, unchanged_offsets=0)
        if window_id in unavailable:
            # The driver found no scroll range on this axis and drove nothing. That is accepted only when the numbers it
            # recorded say so and nothing was requested: content that fits its viewport has no scroll FPS, which is neither
            # a number nor a failure of the instrument. Anything else about it is INVALID.
            said = detail_of(unavailable[window_id])
            extent, viewport = as_float(said.get('extent')), as_float(said.get('viewport'))
            if not mine and extent is not None and viewport is not None and extent - viewport <= 0:
                window.update(applicable=False, status='NOT_APPLICABLE_NO_SCROLL_RANGE', extent=extent, viewport=viewport)
            else:
                window['invalid'].append('NO_SCROLL_EXTENT')
            windows.append(window)
            continue
        if not mine:
            window['invalid'].append('NO_FRAMES_DRAWN')
            windows.append(window)
            continue
        begin = mine[0]['t_ns'] + 500_000_000
        end = begin + int(seconds * 1e9)
        inside = [r for r in mine if begin <= r['t_ns'] < end]
        counted, previous = [], 0.0
        by_seq = {detail_of(r).get('seq'): r for r in mine}
        for request in inside:
            d = detail_of(request)
            frame = frames_by_id.get((window_id, axis, d.get('seq')))
            if frame is None or verdicts.get((window_id, axis, d.get('seq'))) or not (begin <= frame['t_ns'] < end):
                continue
            before = by_seq.get(d.get('seq') - 1)
            previous = as_float(detail_of(before).get('expected_effective_offset')) if before is not None else 0.0
            drawn = as_float(detail_of(frame).get('geometry_drawn_offset'))
            if close_to(drawn, previous, 1e-3):
                window['unchanged_offsets'] += 1
                table.append(dict(window_id=window_id, axis=axis, seq=d.get('seq'), counted=False, reason='UNCHANGED_OFFSET', geometry_drawn_offset=drawn))
                continue
            counted.append(frame)
            f = detail_of(frame)
            wrapped = f['logical_distance'] > f['extent'] - f['viewport']
            window['wrapped_frames'] += 1 if wrapped else 0
            table.append(dict(window_id=window_id, axis=axis, seq=d.get('seq'), counted=True, logical_distance=f['logical_distance'], expected_effective_offset=f['expected_effective_offset'],
                              geometry_drawn_offset=drawn, extent=f['extent'], viewport=f['viewport'], t_ns=frame['t_ns']))
        ordered = sorted(counted, key=lambda f: f['t_ns'])
        intervals = [ms(a['t_ns'], b['t_ns']) for a, b in zip(ordered, ordered[1:])]
        window.update(requested=len(inside), drawn=len(counted), dropped=len(inside) - len(counted), intervals_ms=intervals, window_ns=[begin, end],
                      p99_interval_ms=stats(intervals).get('p99'), intervals_over_33ms=sum(1 for i in intervals if i > 33.3))
        if window['unchanged_offsets']:
            window['invalid'].append('NO_SCROLL_MOVEMENT')
        if not counted:
            window['invalid'].append('NO_FRAMES_DRAWN')
        windows.append(window)
    if windows and not any(w['applicable'] for w in windows):
        reasons.add('NO_SCROLL_EXTENT')
    for window in windows:
        reasons.update(window['invalid'])
    invalid = sorted(reasons)
    for window in windows:
        # Frames per second is a number only for a run that is valid as a whole.
        window['frames_per_second'] = window['drawn'] / seconds if not invalid and window['applicable'] and window['drawn'] else None
    raw = dict(windows=windows, frames=table, ticks=len(names.get('tick', [])), display_max_fps=(names.get('log_opened') or [{}])[0].get('display_max_fps'))
    return outcome_of('analyzed', reasons, raw, dict(frames_per_second={w['window_id']: w['frames_per_second'] for w in windows}), windows=windows)


def analyze_bf7(events, revisions, selected, t0_ns, run_id=None):
    """BF7: an external commit drawn. Each planned revision must have been seen by the model with the task that was
    selected before it, and drawn with that same task, read by the draw, as the one selected."""
    reasons = integrity(events, run_id)
    names = by_name(events)
    seen_by, drawn_by = {}, {}
    for event in names.get('commit_seen', []):
        seen_by.setdefault(event.get('gen'), event)
    for event in names.get('commit_drawn', []):
        drawn_by.setdefault(event.get('gen'), event)
    mismatched = {event.get('gen') for event in names.get('commit_mismatch', [])}
    steps = []
    for revision in revisions:
        seen, drawn = seen_by.get(revision), drawn_by.get(revision)
        step = dict(revision=revision, status='incomplete', installed_in_model_ms=None, drawn_ms_from_cli=None, drawn_ms_since_seen=None)
        started = t0_ns.get(revision)
        if seen is None:
            reasons.add('MISSING_FACT')
        else:
            step['installed_in_model_ms'] = ms(started, seen['t_ns']) if started is not None else None
            if detail_of(seen).get('selected') != selected:
                reasons.add('CONTENT_MISMATCH')
            if started is not None and seen['t_ns'] < started:
                reasons.add('ORDER_VIOLATION')
        if revision in mismatched:
            reasons.add('CONTENT_MISMATCH')
        if drawn is None:
            step['reason'] = 'INCOMPLETE_GENERATION'
            reasons.add('INCOMPLETE_GENERATION')
        else:
            shown = detail_of(drawn)
            step.update(status='drawn', drawn_ms_from_cli=ms(started, drawn['t_ns']) if started is not None else None,
                        drawn_ms_since_seen=ms(seen['t_ns'], drawn['t_ns']) if seen else None, selected_drawn=shown.get('selected'))
            if 'selected' not in shown or 'revision' not in shown:
                reasons.add('MISSING_FACT')
            elif shown['selected'] != selected or str(shown['revision']) != str(revision):
                reasons.add('CONTENT_MISMATCH')
            if started is not None and (drawn['t_ns'] < started or (seen is not None and drawn['t_ns'] < seen['t_ns'])):
                reasons.add('ORDER_VIOLATION')
            if started is not None and late(started, drawn['t_ns'], GENERATION_TIMEOUT * 2):
                reasons.add('LATE_GENERATION')
        steps.append(step)
    values = dict(drawn_ms_from_cli=[s['drawn_ms_from_cli'] for s in steps], drawn_ms_since_seen=[s['drawn_ms_since_seen'] for s in steps],
                  installed_in_model_ms=[s['installed_in_model_ms'] for s in steps])
    return outcome_of('analyzed', reasons, dict(steps=steps), values, steps=steps)


# -- resident memory: the live app and its helper, both measured, or the run is INVALID --------------

def sample_rss(app_pid, helper_pid, run=subprocess.run):
    """One resident-memory sample of the app and its helper by their exact process identities. A sample is valid only when
    both identities are known and `ps` succeeded and returned a positive size for both; anything else is a failure that keeps
    what `ps` was asked and answered, and never a zero for the missing process."""
    sample = dict(ok=False, app=None, helper=None, app_pid=app_pid, helper_pid=helper_pid, reasons=[], argv=None, returncode=None, stdout='', stderr='')
    if not app_pid:
        sample['reasons'].append('APP_PID_UNKNOWN')
    if not helper_pid:
        sample['reasons'].append('HELPER_PID_UNKNOWN')
    if sample['reasons']:
        return sample
    sample['argv'] = ['/bin/ps', '-o', 'pid=,rss=', '-p', f'{app_pid},{helper_pid}']
    try:
        result = run(sample['argv'], capture_output=True, text=True)
    except OSError as error:
        sample['reasons'].append(f'PS_NOT_RUN: {error}')
        return sample
    sample.update(returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)
    if result.returncode != 0:
        sample['reasons'].append('PS_FAILED')
    sizes = {}
    for line in result.stdout.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0].isdigit() and parts[1].isdigit():
            sizes[int(parts[0])] = int(parts[1]) * 1024
        elif parts:
            sample['reasons'].append('PS_UNPARSEABLE')
    for role, pid in (('app', app_pid), ('helper', helper_pid)):
        if pid not in sizes:
            sample['reasons'].append(f'RSS_MISSING_{role.upper()}')
        elif sizes[pid] <= 0:
            sample['reasons'].append(f'RSS_NOT_POSITIVE_{role.upper()}')
        else:
            sample[role] = sizes[pid]
    sample['ok'] = not sample['reasons']
    return sample


def analyze_bf6(events, samples, session_seconds, minimum=10, run_id=None):
    """BF6: resident memory over a session that exercised rendering. Every sample counts only with a successful numeric size
    for the app and for the helper; a missing one, a helper that was replaced mid-session, too few samples or a session that did
    not draw is INVALID, and no peak or growth is then a number."""
    reasons = integrity(events, run_id)
    names = by_name(events)
    exercised = {kind: len(names.get(kind, [])) for kind in ('detail_drawn', 'view_op_drawn', 'frame')}
    if not all(exercised.values()):
        reasons.add('SESSION_DID_NOT_EXERCISE_RENDERING')
    if names.get('view_op_mismatch') or names.get('frame_mismatch'):
        reasons.add('CONTENT_MISMATCH')
    if len(samples) < minimum:
        reasons.add('INSUFFICIENT_SAMPLES')
    if any(not s.get('ok') for s in samples):
        reasons.add('RSS_SAMPLE_INVALID')
    helpers = {s.get('helper_pid') for s in samples if s.get('helper_pid')}
    if len(helpers) > 1:
        reasons.add('HELPER_REPLACED')
    if len({s.get('app_pid') for s in samples}) > 1:
        reasons.add('APP_REPLACED')
    raw = dict(samples=samples, exercised=exercised, session_seconds=session_seconds, sample_count=len(samples), helper_pids=sorted(helpers))
    values = None
    if not reasons:
        totals = [s['app'] + s['helper'] for s in samples]
        m0, m_end = statistics.median(totals[:5]), statistics.median(totals[-5:])
        values = dict(m0_bytes=m0, m_end_bytes=m_end, peak_bytes=max(totals), growth_percent=(m_end - m0) / m0 * 100)
    return outcome_of('analyzed', reasons, raw, values, samples=samples, exercised=exercised, session_seconds=session_seconds, sample_count=len(samples))


# --------------------------------------------------------------------------------------------------
# One measurement per budget (record c.5.1). Each launches, hands the events to the analyzer and returns its verdict.
# --------------------------------------------------------------------------------------------------

class Datasets:
    """The generated plans of a measurement, written once, imported into a fresh store per launch."""

    SPECS = {
        'B100': dict(count=100, shape='branch'), 'B1000': dict(count=1000, shape='branch'), 'B5000': dict(count=5000, shape='branch'),
        'D1000': dict(count=1000, shape='dense'), 'C1000': dict(count=1000, shape='chain'),
    }

    def __init__(self, directory):
        self.directory = Path(directory)
        self.files = {}
        self.records = {}
        self.stores = 0

    def need(self, name):
        if name not in self.files:
            spec = self.SPECS[name]
            plan = dataset(spec['count'], spec['shape'])
            path = write_plan(self.directory / f'{name}.json', plan)
            self.files[name] = path
            self.records[name] = dict(describe(plan), name=name, count=spec['count'], shape=spec['shape'], hierarchy='DS-H',
                                      file_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
        return self.records[name]

    def store(self, name):
        self.need(name)
        self.stores += 1
        database = self.directory / f'{name}-{self.stores}.sqlite'
        run_cli(database, 'import', str(self.files[name]))
        return database

    def keys(self, name, count=10):
        """Ten different tasks at indexes (2k+1)*N//20, k = 0..9, as PERF keys."""
        n = self.SPECS[name]['count']
        return [f'PERF-{(2 * k + 1) * n // 20 + 1}' for k in range(count)]


def bf1(bundle, directory, datasets, name):
    database = datasets.store(name)
    expected = datasets.need(name)['rows_expanded']
    run = Launched(bundle, directory, f'bf1-{name}', ['--database', str(database), '--page', 'gantt'])
    try:
        run.wait_event('first_draw', FIRST_DRAW_TIMEOUT)
        time.sleep(0.5)  # a pause by design: the draw that follows the first one is part of the log
        run.quit()
        return analyze_bf1(run.events, expected, run.t0, run.measure_id)
    finally:
        run.close()


def bf2(bundle, directory, datasets, name):
    database = datasets.store(name)
    keys = datasets.keys(name)
    run = Launched(bundle, directory, f'bf2-{name}', ['--database', str(database), '--page', 'detail', '--measure-script', 'selections', '--measure-keys', ','.join(keys)])
    try:
        if run.wait_event('driver_done', 180) is None:
            raise RunFailed('bf2: the selection driver did not finish')
        state = run.state()
        run.quit()
        result = analyze_bf2(run.events, keys, run.measure_id)
        result['steady_max_gap_ms'] = state['main_thread']['steady_max_gap_ms'] if state else None
        return result
    finally:
        run.close()


VIEW_KINDS = list(PLANNED_VIEW_OPS)


def bf4(bundle, directory, datasets, name):
    database = datasets.store(name)
    record = datasets.need(name)
    run = Launched(bundle, directory, f'bf4-{name}', ['--database', str(database), '--page', 'gantt', '--measure-script', 'viewops'])
    try:
        if run.wait_event('driver_done', 300) is None:
            raise RunFailed('bf4: the view-operation driver did not finish')
        state = run.state()
        run.quit()
        result = analyze_bf4(run.events, record, run.measure_id)
        result['steady_max_gap_ms'] = state['main_thread']['steady_max_gap_ms'] if state else None
        return result
    finally:
        run.close()


def bf5(bundle, directory, datasets, name, seconds=5.0):
    database = datasets.store(name)
    run = Launched(bundle, directory, f'bf5-{name}', ['--database', str(database), '--page', 'gantt', '--measure-script', 'scroll', '--measure-seconds', str(seconds)])
    try:
        if run.wait_event('driver_done', 120) is None:
            raise RunFailed('bf5: the scroll driver did not finish')
        state = run.state()
        run.quit()
        result = analyze_bf5(run.events, seconds, run_id=run.measure_id)
        result['steady_max_gap_ms'] = state['main_thread']['steady_max_gap_ms'] if state else None
        return result
    finally:
        run.close()


def bf6(bundle, directory, datasets, name, session_seconds):
    database = datasets.store(name)
    keys = datasets.keys(name, 5)
    delay = 16
    run = Launched(bundle, directory, f'bf6-{name}', ['--database', str(database), '--page', 'gantt', '--select-key', 'PERF-1', '--measure-script', 'session',
                                                    '--measure-seconds', str(session_seconds), '--measure-keys', ','.join(keys), '--measure-delay', str(delay)])
    try:
        run.wait_state(lambda s: loaded(s) and s['selection'] and s['detail'] and not s['detail']['loading'], 120, 'the plan loaded with its detail')
        start = time.monotonic()
        samples = []
        next_sample = start + 10
        limit = start + delay + session_seconds + 180
        while True:
            now = time.monotonic()
            if now >= next_sample:
                # The sampling cadence is the measurement's own clock; what is waited on in between is events.
                state = run.state() or {}
                alive = run.process.poll() is None
                sample = sample_rss(run.process.pid if alive and state.get('pid') == run.process.pid else None, state.get('helper_pid'))
                sample['t'] = now - start
                samples.append(sample)
                next_sample = max(next_sample + 1.0, time.monotonic())
            run.pump()
            if run.by_name.get('driver_done'):
                break
            if run.process.poll() is not None or now > limit:
                raise RunFailed('bf6: the session did not complete')
            run.wait_activity(max(0.0, min(next_sample, limit) - time.monotonic()))
        run.quit()
        return analyze_bf6(run.events, samples, session_seconds, run_id=run.measure_id)
    finally:
        run.close()


def commit_probe(bundle, directory, datasets, name, steps, freeze=(), label='bf7'):
    """The external-commit scenario: a started task selected in the app, then `steps` progress commits by the CLI. Returns the
    launched run and what the analyzer needs: the revisions expected, the key selected before, and when each commit was made."""
    database = datasets.store(name)
    run_cli(database, 'claim', 'PERF-1', '--actor', WORKER)
    run_cli(database, 'start', 'PERF-1', '--actor', WORKER)
    run = Launched(bundle, directory, f'{label}-{name}', ['--database', str(database), '--page', 'gantt', '--select-key', 'PERF-1'], freeze=freeze)
    try:
        state = run.wait_state(lambda s: loaded(s) and s['selection'] and s['detail'] and not s['detail']['loading'], 120, 'the plan loaded with PERF-1 selected')
        if not freeze and run.wait_event('first_draw', FIRST_DRAW_TIMEOUT) is None:
            raise RunFailed(f'{label}: the first draw was not reached')
        selected = state['selection']['key']
        revision, revisions, started = state['revision'], [], {}
        for percent in steps:
            revision += 1
            started[revision] = uptime_ns()
            run_cli(database, 'progress', 'PERF-1', str(percent), '--actor', WORKER)
            revisions.append(revision)
            run.wait_event('commit_seen', GENERATION_TIMEOUT * 2, gen=revision)
            run.wait_event('commit_drawn', GENERATION_TIMEOUT * 2, gen=revision)
        return run, revisions, selected, started
    except BaseException:
        run.close()
        raise


def bf7(bundle, directory, datasets, name):
    run, revisions, selected, started = commit_probe(bundle, directory, datasets, name, (20, 40, 60, 80, 100))
    try:
        run.quit()
        return analyze_bf7(run.events, revisions, selected, started, run.measure_id)
    finally:
        run.close()


# --------------------------------------------------------------------------------------------------
# The controls (record c.5.0): withhold the draws, let everything else carry on, and then let the very same
# analyzer judge the log. Frozen logs, a displaced coordinate and mutated copies of a real log must all be
# INVALID with no number; the paired real logs must be valid.
# --------------------------------------------------------------------------------------------------

def counted(run, names):
    counts = {n: len(v) for n, v in run.by_name.items()}
    return {k: counts.get(k, 0) for k in names}


def control_variant(bundle, directory, datasets, name, freeze, arguments, driver=True, wait=4.0, extra=()):
    database = datasets.store('B100')
    run = Launched(bundle, directory, f'control-{name}', ['--database', str(database), *arguments], freeze=freeze)
    try:
        if driver:
            run.wait_event('driver_done', 120)
        else:
            run.wait_state(lambda s: s['connection'] == 'connected', 60, 'the store to connect')
            time.sleep(wait)  # the control's own window: nothing may be drawn within it
        run.quit()
        run.pump()
        verdict = dict(name=name, frozen=list(freeze), counts=counted(run, ('tick', 'state_set', 'select_call', 'view_op_call', 'scroll_request', 'commit_seen', 'first_draw', 'detail_drawn',
                                                                           'view_op_drawn', 'frame', 'commit_drawn', 'frame_mismatch')))
        return verdict, run
    finally:
        run.close()


def judged(verdict, analysis):
    """What the analyzer said of a control's log, kept with the control."""
    verdict['analysis'] = dict(invalid=analysis['invalid'], numbers_present=analysis['values'] is not None)
    return analysis


def first_of(events, name, usable=lambda e: True):
    """The first event of a retained log that can carry a deliberate fault; a log without one cannot be replayed, and that is said."""
    found = next((e for e in events if e.get('event') == name and usable(e)), None)
    if found is None:
        raise RunFailed(f'the retained log has no {name} event to carry a deliberate fault, so the replay cannot be made')
    return found


def mutate(events, how, gen=None):
    """A copy of a retained log with one deliberate fault, for replay through the analyzer. `gen` names the generation a
    fault must fall on when the log holds others (a commit of the load itself) that no measurement asks about."""
    events = copy.deepcopy(events)
    if how == 'content_mismatch_frame':
        first_of(events, 'frame')['detail']['geometry_drawn_offset'] += 5.0
    elif how == 'content_mismatch_zoom':
        drawn = first_of(events, 'view_op_drawn', lambda e: e['detail'].get('zoom') is not None)
        drawn['detail']['zoom'] = str(int(drawn['detail']['zoom']) + 1)
    elif how == 'dropped_event':
        events.remove(first_of(events, 'view_op_drawn'))
        closing = first_of(events, 'log_closed')
        closing['dropped'] = closing.get('dropped', 0) + 1
    elif how == 'late_generation':
        first_of(events, 'view_op_drawn')['t_ns'] += int((GENERATION_TIMEOUT + 1) * 1e9)
    elif how == 'timeout_then_draw':
        call = first_of(events, 'view_op_call')
        events.insert(len(events) - 1, dict(event='generation_timeout', gen=call['gen'], t_ns=call['t_ns'] + 1, detail=dict(kind='view_op'), measure_id=call['measure_id'], pid=call['pid']))
    elif how == 'missing_operation':
        call = first_of(events, 'view_op_call')
        events[:] = [e for e in events if not (e.get('event') in ('view_op_call', 'state_set', 'view_op_drawn') and e.get('gen') == call['gen'] and (e.get('event') != 'state_set' or e['detail'].get('kind') == 'view_op'))]
    elif how == 'wrong_selection':
        first_of(events, 'commit_drawn', lambda e: gen is None or e.get('gen') == gen)['detail']['selected'] = 'PERF-2'
    return events


def controls(bundle, directory, datasets):
    """NC-0, the paired positive controls, NC-1a, b, d, e, f and g, and the replay of mutated real logs. NC-1c (operation outcome)
    belongs to the operations panel, which this read-only observer does not have, so it is not applicable here."""
    results, failures = [], []

    def record(verdict, ok, why):
        verdict.update(control_verdict='CONTROL_PASS' if ok else 'CONTROL_FAIL', reason=None if ok else why)
        results.append(verdict)
        if not ok:
            failures.append(f"{verdict['name']}: {why}")

    def invalid_without_numbers(analysis, *reasons):
        return all(r in analysis['invalid'] for r in reasons) and analysis['values'] is None

    b100 = datasets.need('B100')
    keys3 = datasets.keys('B100', 3)
    # NC-0: the same scenarios with every gate open must complete generations and frames, and the analyzer must call them valid.
    verdict, run = control_variant(bundle, directory, datasets, 'NC-0-detail', (), ['--page', 'detail', '--measure-script', 'selections', '--measure-keys', ','.join(keys3)])
    analysis = judged(verdict, analyze_bf2(run.events, keys3, run.measure_id))
    record(verdict, verdict['counts']['detail_drawn'] == 3 and verdict['counts']['tick'] > 0 and not analysis['invalid'] and analysis['values'] is not None, 'with the gate open, every selection must complete and the analyzer must find the run valid')
    verdict, run = control_variant(bundle, directory, datasets, 'NC-0-viewops', (), ['--page', 'gantt', '--measure-script', 'viewops'])
    analysis = judged(verdict, analyze_bf4(run.events, b100, run.measure_id))
    viewops_events = list(run.events)
    record(verdict, verdict['counts']['view_op_drawn'] == len(VIEWOPS_SCRIPT) * VIEWOPS_ROUNDS and not analysis['invalid'] and analysis['values'] is not None,
           'with the gate open, every planned view operation must complete with its expected state and the analyzer must find the run valid')
    verdict, run = control_variant(bundle, directory, datasets, 'NC-0-gantt', (), ['--page', 'gantt', '--measure-script', 'scroll', '--measure-seconds', '2'])
    analysis = judged(verdict, analyze_bf5(run.events, 2.0, run_id=run.measure_id))
    scroll_events = list(run.events)
    scrolling = [w for w in analysis['windows'] if w['applicable']]
    verdict['windows'] = [dict(axis=w['axis'], window_id=w['window_id'], applicable=w['applicable'], drawn=w['drawn'], frames_per_second=w['frames_per_second'], wrapped_frames=w['wrapped_frames']) for w in analysis['windows']]
    record(verdict, verdict['counts']['first_draw'] == 1 and verdict['counts']['frame'] > 0 and not analysis['invalid'] and analysis['values'] is not None
           and bool(scrolling) and all(w['drawn'] > 0 and w['frames_per_second'] for w in scrolling), 'with the gate open, the first draw and frames must exist and the analyzer must find the run valid')
    verdict, run = control_variant(bundle, directory, datasets, 'NC-0-repeat', (), ['--page', 'gantt', '--measure-script', 'scroll-repeat', '--measure-seconds', '2'])
    analysis = judged(verdict, analyze_bf5(run.events, 2.0, run_id=run.measure_id))
    ids = [(w['window_id'], w['axis']) for w in analysis['windows']]
    verdict['windows'] = [dict(axis=w['axis'], window_id=w['window_id'], applicable=w['applicable'], drawn=w['drawn']) for w in analysis['windows']]
    record(verdict, len(ids) == 4 and len(set(ids)) == 4 and not analysis['invalid'] and any(w['applicable'] for w in analysis['windows'])
           and all(w['drawn'] > 0 for w in analysis['windows'] if w['applicable']),
           'two axes in two successive windows of one process must have unique generation identities and valid frames in every window')
    commit_run, revisions, selected, started = commit_probe(bundle, directory, datasets, 'B100', (20,), label='control-NC-0-commit')
    try:
        commit_run.quit()
        analysis = analyze_bf7(commit_run.events, revisions, selected, started, commit_run.measure_id)
        commit_events = list(commit_run.events)
    finally:
        commit_run.close()
    verdict = dict(name='NC-0-commit', frozen=[], counts=counted(commit_run, ('commit_seen', 'commit_drawn', 'commit_mismatch', 'tick')))
    judged(verdict, analysis)
    record(verdict, not analysis['invalid'] and analysis['values'] is not None and verdict['counts']['commit_drawn'] >= 1, 'with the gate open, the commit must be drawn with the selected task and the analyzer must find the run valid')
    # NC-1a: gantt frozen from launch: no first draw, ticks continue.
    verdict, run = control_variant(bundle, directory, datasets, 'NC-1a', ('gantt',), ['--page', 'gantt'], driver=False, wait=FIRST_DRAW_TIMEOUT / 2)
    analysis = judged(verdict, analyze_bf1(run.events, b100['rows_expanded'], run.t0, run.measure_id))
    record(verdict, verdict['counts']['first_draw'] == 0 and verdict['counts']['tick'] > 0 and invalid_without_numbers(analysis, 'FIRST_DRAW_NOT_REACHED'),
           'a frozen Gantt must not reach a first draw while ticks continue, and the analyzer must call it INVALID with no number')
    # NC-1b: detail frozen: ten selections called, the model advanced, nothing drawn.
    keys10 = datasets.keys('B100')
    verdict, run = control_variant(bundle, directory, datasets, 'NC-1b', ('detail',), ['--page', 'detail', '--measure-script', 'selections', '--measure-keys', ','.join(keys10)])
    analysis = judged(verdict, analyze_bf2(run.events, keys10, run.measure_id))
    record(verdict, verdict['counts']['select_call'] == 10 and verdict['counts']['state_set'] >= 1 and verdict['counts']['detail_drawn'] == 0 and verdict['counts']['tick'] > 0
           and invalid_without_numbers(analysis, 'INCOMPLETE_GENERATION'), 'a frozen Detail must show ten calls, the model advanced, no draw and ticks, and the analyzer must call it INVALID with no number')
    # NC-1d: gantt frozen: view operations called and applied, none drawn.
    verdict, run = control_variant(bundle, directory, datasets, 'NC-1d', ('gantt',), ['--page', 'gantt', '--measure-script', 'viewops'])
    analysis = judged(verdict, analyze_bf4(run.events, b100, run.measure_id))
    record(verdict, verdict['counts']['view_op_call'] > 0 and verdict['counts']['state_set'] > 0 and verdict['counts']['view_op_drawn'] == 0 and verdict['counts']['tick'] > 0
           and invalid_without_numbers(analysis, 'INCOMPLETE_GENERATION'), 'a frozen Gantt must call and apply view operations and draw none, and the analyzer must call it INVALID with no number')
    # NC-1e: gantt frozen through a scroll window: requests, ticks and no frame.
    verdict, run = control_variant(bundle, directory, datasets, 'NC-1e', ('gantt',), ['--page', 'gantt', '--measure-script', 'scroll', '--measure-seconds', '2'])
    analysis = judged(verdict, analyze_bf5(run.events, 2.0, run_id=run.measure_id))
    verdict['windows'] = [dict(axis=w['axis'], applicable=w['applicable'], requested=w['requested'], drawn=w['drawn'], dropped=w['dropped'], frames_per_second=w['frames_per_second'], invalid=w['invalid']) for w in analysis['windows']]
    scrolling = [w for w in analysis['windows'] if w['applicable']]
    record(verdict, bool(scrolling) and all(w['requested'] > 0 and w['drawn'] == 0 and w['dropped'] == w['requested'] and 'NO_FRAMES_DRAWN' in w['invalid'] and w['frames_per_second'] is None for w in scrolling)
           and verdict['counts']['tick'] > 0 and invalid_without_numbers(analysis, 'NO_FRAMES_DRAWN'),
           'a frozen Gantt must request frames, draw none and drop every request, and the analyzer must call it INVALID with no frames per second')
    # NC-1f: gantt frozen, an external commit: the model sees the revision, nothing draws it.
    frozen_run, frozen_revisions, frozen_selected, frozen_started = commit_probe(bundle, directory, datasets, 'B100', (20,), freeze=('gantt',), label='control-NC-1f')
    try:
        frozen_run.quit()
        analysis = analyze_bf7(frozen_run.events, frozen_revisions, frozen_selected, frozen_started, frozen_run.measure_id)
    finally:
        frozen_run.close()
    verdict = dict(name='NC-1f', frozen=['gantt'], counts=counted(frozen_run, ('commit_seen', 'commit_drawn', 'state_set', 'tick')))
    judged(verdict, analysis)
    record(verdict, verdict['counts']['commit_seen'] >= 1 and verdict['counts']['commit_drawn'] == 0 and verdict['counts']['tick'] > 0 and invalid_without_numbers(analysis, 'INCOMPLETE_GENERATION'),
           'a frozen Gantt must see the commit in the model and never draw it, and the analyzer must call it INVALID with no number')
    # NC-1g: the coordinate the content is drawn at is displaced: every frame must be a mismatch, and no frames per second exist.
    verdict, run = control_variant(bundle, directory, datasets, 'NC-1g', (), ['--page', 'gantt', '--measure-script', 'scroll', '--measure-seconds', '2', '--measure-displace', '7'])
    analysis = judged(verdict, analyze_bf5(run.events, 2.0, run_id=run.measure_id))
    verdict['windows'] = [dict(axis=w['axis'], requested=w['requested'], drawn=w['drawn'], frames_per_second=w['frames_per_second'], invalid=w['invalid']) for w in analysis['windows']]
    record(verdict, verdict['counts']['frame'] == 0 and verdict['counts']['frame_mismatch'] > 0 and invalid_without_numbers(analysis, 'CONTENT_MISMATCH')
           and all(w['frames_per_second'] is None for w in analysis['windows']),
           'content drawn 7 points from where it was requested must be refused as CONTENT_MISMATCH and yield no frames per second')
    # Replay: mutated copies of the retained real logs go through the same analyzer and must be INVALID with no number.
    replays = [('NC-R-frame-offset', analyze_bf5, scroll_events, 'content_mismatch_frame', ('CONTENT_MISMATCH',), lambda e: analyze_bf5(e, 2.0)),
               ('NC-R-zoom', analyze_bf4, viewops_events, 'content_mismatch_zoom', ('CONTENT_MISMATCH',), lambda e: analyze_bf4(e, b100)),
               ('NC-R-dropped-event', analyze_bf4, viewops_events, 'dropped_event', ('EVENTS_DROPPED', 'INCOMPLETE_GENERATION'), lambda e: analyze_bf4(e, b100)),
               ('NC-R-late-generation', analyze_bf4, viewops_events, 'late_generation', ('LATE_GENERATION',), lambda e: analyze_bf4(e, b100)),
               ('NC-R-timeout-then-draw', analyze_bf4, viewops_events, 'timeout_then_draw', ('GENERATION_TIMEOUT',), lambda e: analyze_bf4(e, b100)),
               ('NC-R-missing-operation', analyze_bf4, viewops_events, 'missing_operation', ('MISSING_REQUIRED_OPERATION',), lambda e: analyze_bf4(e, b100)),
               ('NC-R-selection', analyze_bf7, commit_events, 'wrong_selection', ('CONTENT_MISMATCH',), lambda e: analyze_bf7(e, revisions, selected, started))]
    for name, _, base, how, reasons, judge in replays:
        verdict = dict(name=name, replay_of='retained real log with a deliberate fault', fault=how)
        try:
            analysis = judge(mutate(base, how, gen=revisions[0] if how == 'wrong_selection' else None))
        except RunFailed as error:
            # The retained log has no frame/draw to carry the fault (for instance no tick or draw ever arrived): the control
            # could not run. It is SKIPPED with the reason, never passed, and the controls as a whole cannot pass.
            verdict.update(control_verdict='SKIPPED', reason=str(error))
            results.append(verdict)
            failures.append(f"{name}: SKIPPED, not passed: {error}")
            continue
        verdict['analysis'] = dict(invalid=analysis['invalid'], numbers_present=analysis['values'] is not None)
        record(verdict, invalid_without_numbers(analysis, *reasons), f'the log with the fault {how} must be INVALID ({", ".join(reasons)}) with no number; the analyzer said {analysis["invalid"]}')
    results.append(dict(name='NC-1c', control_verdict='NOT_APPLICABLE', reason='operation outcomes belong to the operations panel (FEAT-40); the read-only observer has none'))
    return dict(results=results, verdict='CONTROL_PASS' if not failures else 'CONTROL_FAIL', failures=failures), failures


# --------------------------------------------------------------------------------------------------
# The whole measurement.
# --------------------------------------------------------------------------------------------------

TARGETS = {
    'contract': 'live contract of FEAT-20, acceptance 4: 100- and 1000-task generated plans',
    'BF1_first_draw_ms': dict(contract_max=1000, design_proposal_median=500, scale_probe_5000_median=3000),
    'BF2_selection_to_detail_ms': dict(contract_max=250, design_proposal_median=100),
    'BF3_operation_outcome_ms': dict(not_measured='belongs to the operations panel (FEAT-40); the read-only observer has no operation'),
    'BF4_view_operation_ms': dict(design_proposal_median=100, design_proposal_max=250),
    'BF5_frames_per_second': dict(contract_min=30, design_proposal_median=55, design_proposal_p99_interval_ms=33.3),
    'BF6_resident_memory': dict(contract_peak_max_bytes=300_000_000, contract_peak_max_mib=286.10, design_proposal_growth_percent=10, scale_probe_5000_peak_max_bytes=600_000_000),
    'BF7_external_commit_drawn_ms': dict(design_proposal_median=1000, design_proposal_max=2000),
    'BF8_steady_max_gap_ms': dict(design_proposal_max=50),
    'BF9_readability': dict(not_measured='a reviewer checks it (J5.3); it is not a timing'),
}

SCROLL_METHOD = ('FEAT-20 measurement method, a bounded clarification the root approved for FEAT-20 configuration and evidence only (the sealed FEAT-05 files are unchanged): '
                 'the content is finite, so a scroll request carries logical_distance = 40 points x its number in the window and the content wraps over its scroll range '
                 '(extent less viewport); the DRIVER works out the expected effective offset apart from the draw; the draw stamps the geometry_drawn_offset the content was actually placed at. '
                 'Per frame the log records logical_distance, expected_effective_offset, geometry_drawn_offset, axis, window_id, extent and viewport. A frame counts only for a unique requested '
                 'generation (window, axis, number), only if the offset moved and expected equals drawn; a content with no scroll range is not scroll FPS. '
                 'Unchanged: 5.0 s windows after a discarded 0.5 s, the warm-up, the repetitions, the 30 frames/s target, draw completion (not display presentation) as the meaning of a frame, '
                 'the negative controls and the INVALID rules.')


def repeat(label, work, failures, repetitions):
    """One discarded warm-up, then the kept repetitions. A repetition that did not complete, or that the analyzer finds INVALID,
    is recorded with its reason and counted as a failure of the measurement, never skipped. Every launch it made is named in it."""
    outcomes = []
    for index in range(repetitions + 1):
        mark = len(CAPTURE.launches)
        try:
            outcome = work()
        except RunFailed as error:
            outcome = dict(status='failed', reason=str(error), invalid=['RUN_DID_NOT_COMPLETE'], raw={}, values=None)
            for launch in CAPTURE.launches[mark:]:
                launch.finalize(failure=str(error))
        outcome['warm_up'] = index == 0
        outcome['repetition'] = index
        outcome['evidence'] = [launch.evidence() for launch in CAPTURE.launches[mark:]]
        outcomes.append(outcome)
        if outcome['invalid']:
            failures.append(f'{label} repetition {index}{" (warm-up)" if index == 0 else ""} is INVALID: {",".join(outcome["invalid"])}' + (f' ({outcome["reason"]})' if outcome.get('reason') else ''))
        print(f'  {label} repetition {index}{" (warm-up, discarded)" if index == 0 else ""}: {"INVALID " + ",".join(outcome["invalid"]) if outcome["invalid"] else "valid"}', flush=True)
    return outcomes


def kept(outcomes):
    return [o for o in outcomes if not o['warm_up']]


def qualified(outcomes, repetitions):
    """An aggregate is a number only when every kept repetition is valid and all of them were made."""
    run = kept(outcomes)
    return len(run) == repetitions and all(not o['invalid'] for o in run) and all(o['values'] is not None for o in run)


def reasons_of(outcomes):
    return sorted({r for o in kept(outcomes) for r in o['invalid']})


def aggregate_bf1(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    values = [o['values'] for o in kept(outcomes)] if ok else []
    spawn = [v['first_draw_ms_from_spawn'] for v in values]
    entry = [v['first_draw_ms_since_process_entry'] for v in values if v['first_draw_ms_since_process_entry'] is not None]
    return dict(qualified=ok, invalid=reasons_of(outcomes), raw_from_spawn_ms=spawn, raw_since_process_entry_ms=entry,
                from_spawn=stats(spawn) if ok else None, since_process_entry=stats(entry) if ok else None)


def aggregate_bf2(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    per = [o['values']['kept_latency_ms'] for o in kept(outcomes)] if ok else []
    flat = [v for each in per for v in each]
    return dict(qualified=ok, invalid=reasons_of(outcomes), raw_ms=flat, all_values=stats(flat) if ok else None, per_repetition=[stats(each) for each in per])


def aggregate_bf4(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    result = dict(qualified=ok, invalid=reasons_of(outcomes))
    for kind in VIEW_KINDS:
        values = [v for o in kept(outcomes) for v in (o['values'] or {}).get(kind, [])] if ok else []
        result[kind] = dict(raw_ms=values, all_values=stats(values) if ok else None)
    return result


def aggregate_bf5(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    result = dict(qualified=ok, invalid=reasons_of(outcomes))
    for axis in ('vertical', 'horizontal'):
        windows = [w for o in kept(outcomes) for w in o['raw'].get('windows', []) if w['axis'] == axis]
        applicable = [w for w in windows if w.get('applicable', True)]
        values = [w['frames_per_second'] for w in applicable] if ok else []
        # An axis whose content fits its viewport has no scroll FPS: no number, and not an invalid instrument either.
        not_applicable = bool(windows) and not applicable
        result[axis] = dict(raw_frames_per_second=values, stats=stats(values) if ok and values else None, status='NOT_APPLICABLE_NO_SCROLL_RANGE' if not_applicable else None,
                            content_vs_viewport=[(w.get('extent'), w.get('viewport')) for w in windows if not w.get('applicable', True)],
                            requested=[w['requested'] for w in windows], drawn=[w['drawn'] for w in windows],
                            dropped=[w['dropped'] for w in windows], wrapped_frames=[w['wrapped_frames'] for w in windows], p99_interval_ms=[w.get('p99_interval_ms') for w in windows] if ok else [],
                            intervals_over_33ms=[w.get('intervals_over_33ms') for w in windows] if ok else [],
                            interval_distribution_ms=stats([i for w in windows for i in w['intervals_ms']]) if ok else None)
    return result


def aggregate_bf6(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    mib = 1048576
    values = [o['values'] for o in kept(outcomes)] if ok else []
    peaks = [v['peak_bytes'] for v in values]
    return dict(qualified=ok, invalid=reasons_of(outcomes), peaks_bytes=peaks, peaks_mib=[round(p / mib, 2) for p in peaks], m0_mib=[round(v['m0_bytes'] / mib, 2) for v in values],
                m_end_mib=[round(v['m_end_bytes'] / mib, 2) for v in values], growth_percent=[round(v['growth_percent'], 2) for v in values],
                peak_max_bytes=max(peaks, default=None), peak_max_mib=round(max(peaks) / mib, 2) if peaks else None)


def aggregate_bf7(outcomes, repetitions):
    ok = qualified(outcomes, repetitions)
    values = [o['values'] for o in kept(outcomes)] if ok else []
    drawn = [v for each in values for v in each['drawn_ms_from_cli']]
    since = [v for each in values for v in each['drawn_ms_since_seen']]
    model = [v for each in values for v in each['installed_in_model_ms']]
    return dict(qualified=ok, invalid=reasons_of(outcomes), raw_drawn_ms_from_cli=drawn, drawn_from_cli=stats(drawn) if ok else None, drawn_since_model_install=stats(since) if ok else None,
                installed_in_model_not_drawn=stats(model) if ok else None)


def aggregate_bf8(outcomes):
    gaps = [o['steady_max_gap_ms'] for o in kept(outcomes) if not o['invalid'] and o.get('steady_max_gap_ms') is not None]
    return dict(raw_steady_max_gap_ms=gaps, stats=stats(gaps), qualified=bool(gaps) and len(gaps) == len(kept(outcomes)))


def withhold(measured, control):
    """A failed control means no BF number may be kept: every aggregate is marked not qualified and its statistics are removed."""
    if control['verdict'] == 'CONTROL_PASS':
        return
    for key, by_dataset in measured.items():
        if not isinstance(by_dataset, dict):
            continue
        for name, entry in by_dataset.items():
            if isinstance(entry, dict) and 'aggregate' in entry:
                entry['aggregate'] = dict(qualified=False, withheld='CONTROL_FAIL', reason='the negative control failed, so no number may be kept (record J5.5)', invalid=entry['aggregate'].get('invalid', []))
            elif isinstance(entry, dict):
                measured[key][name] = dict(qualified=False, withheld='CONTROL_FAIL')


def gates(measured, control):
    """Each contract threshold against its aggregate, kept apart from the validity of the instrument: INVALID or withheld
    aggregates are not evaluated, never a miss. Only the contract's own thresholds are applied; design proposals are only reported."""
    result = {}

    def verdict(aggregate, test):
        if control['verdict'] != 'CONTROL_PASS':
            return 'NOT_EVALUATED_CONTROL_FAIL'
        if not aggregate.get('qualified'):
            return 'NOT_EVALUATED_INVALID'
        return 'MEETS_TARGET' if test(aggregate) else 'MISSES_TARGET'

    for name, entry in measured.get('BF1', {}).items():
        if isinstance(entry, dict) and 'aggregate' in entry:
            result[f'BF1 {name}: max first draw <= 1000 ms'] = verdict(entry['aggregate'], lambda a: a['from_spawn']['max'] <= 1000)
    for name, entry in measured.get('BF2', {}).items():
        if isinstance(entry, dict) and 'aggregate' in entry:
            result[f'BF2 {name}: max selection to Detail <= 250 ms'] = verdict(entry['aggregate'], lambda a: a['all_values']['max'] <= 250)
    for name, entry in measured.get('BF5', {}).items():
        if isinstance(entry, dict) and 'aggregate' in entry:
            for axis in ('vertical', 'horizontal'):
                label = f'BF5 {name} {axis}: min frames/s >= 30'
                if control['verdict'] == 'CONTROL_PASS' and entry['aggregate'].get('qualified') and entry['aggregate'][axis]['status']:
                    result[label] = 'NOT_APPLICABLE_NO_SCROLL_RANGE'
                else:
                    result[label] = verdict(entry['aggregate'], lambda a, axis=axis: a[axis]['stats']['min'] >= 30)
    for name, entry in measured.get('BF6', {}).items():
        if isinstance(entry, dict) and 'aggregate' in entry:
            result[f'BF6 {name}: max peak < 300000000 bytes'] = verdict(entry['aggregate'], lambda a: a['peak_max_bytes'] < 300_000_000)
    return result


def render_dense(bundle, directory, datasets, out):
    """The app renders the dense 1000-task plan to PNG, only when DPM_RENDER_OUT names a directory: app-rendered,
    scripted and synthetic, not an operator observation. Nothing is written when it is not set."""
    out.mkdir(parents=True, exist_ok=True)
    database = datasets.store('D1000')
    names = [f'gantt-dense-1000-{kind}.png' for kind in ('expanded', 'collapsed', 'selected-nested-detail')]
    run = Launched(bundle, directory, 'render-dense', ['--database', str(database), '--page', 'gantt', '--render-to', str(out), '--render-prefix', 'gantt-dense-1000', '--render-when', 'loaded'])
    try:
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline and not all((out / n).is_file() for n in names):
            if run.process.poll() is not None:
                break
            run.wait_activity(0.3)  # the app's own writes into its directory wake this; the output directory is polled at this bound
        time.sleep(0.5)
        missing = [n for n in names if not (out / n).is_file()]
        if missing:
            raise RunFailed(f'render: {missing} were not written')
        run.quit()
        return [dict(name=n, bytes=(out / n).stat().st_size, sha256=hashlib.sha256((out / n).read_bytes()).hexdigest(), label='app-rendered, scripted, synthetic; not an operator observation') for n in names]
    finally:
        run.close()


def hardware():
    def sysctl(name):
        return subprocess.run(['/usr/sbin/sysctl', '-n', name], capture_output=True, text=True).stdout.strip()
    versions = subprocess.run(['/usr/bin/sw_vers'], capture_output=True, text=True).stdout.split('\n')
    return dict(model=sysctl('hw.model'), cpu=sysctl('machdep.cpu.brand_string'), logical_cpus=sysctl('hw.logicalcpu'), memory_bytes=sysctl('hw.memsize'),
                sw_vers=[line.strip() for line in versions if line.strip()], machine=platform.machine(), python=platform.python_version())


# --------------------------------------------------------------------------------------------------
# The self-test: the analyzer and the capture on frozen logs. The logs here are SYNTHETIC fixtures written to the
# schema of the app's log; they are not draws of the app and prove nothing about it. They show the analyzer is
# able to say INVALID, to give a number to a valid log and to give none to an invalid one, so that the real
# controls above (which replay the app's own retained logs) and the real measurement are judged by something that can fail.
# --------------------------------------------------------------------------------------------------

class Synthetic:
    """A builder of a frozen log, in the app's own schema."""

    def __init__(self, run_id='run-1', pid=4242):
        self.run_id, self.pid, self.clock = run_id, pid, 1_000_000_000
        self.events = []
        self.add('log_opened', detail={}, hardware_model='synthetic', display_max_fps=60)
        self.add('process_entry')

    def add(self, event, gen=None, detail=None, step_ms=1.0, **more):
        self.clock += int(step_ms * 1e6)
        item = dict(event=event, measure_id=self.run_id, pid=self.pid, t_ns=self.clock, detail=detail or {})
        if gen is not None:
            item['gen'] = gen
        item.update(more)
        self.events.append(item)
        return item

    def ticks(self, count=5):
        for n in range(count):
            self.add('tick', detail=dict(n=n))

    def close(self, dropped=0):
        self.events.append(dict(event='log_closed', dropped=dropped, ignored_stamps=0, measure_id=self.run_id, t_ns=self.clock + 1))
        return self.events


RECORD = dict(tasks=100, packages=10, rows_expanded=110, rows_collapsed=10)


def synthetic_viewops(record=RECORD, broken=None):
    """The five blocks of the `viewops` script as a log of a correct run: each call on entry, the state it expects after, and its draw."""
    log = Synthetic()
    log.ticks()
    expectation = planned_views(record)
    sequence = 0
    pan = '0.000'
    for round_ in range(VIEWOPS_ROUNDS):
        for slot, (kind, want) in enumerate(expectation[round_ * len(VIEWOPS_SCRIPT):(round_ + 1) * len(VIEWOPS_SCRIPT)]):
            sequence += 1
            log.add('view_op_call', gen=sequence, detail=dict(op=kind))
            rows = want.get('rows', str(record['rows_expanded']))
            if kind == 'pan':
                pan = '160.000' if slot == VIEWOPS_SCRIPT.index('pan') else '0.000'
            first_filter = VIEWOPS_SCRIPT.index('filter')
            last_filter = len(VIEWOPS_SCRIPT) - 1 - VIEWOPS_SCRIPT[::-1].index('filter')
            filter_word = 'critical only' if first_filter <= slot < last_filter else 'no filter'
            facts = dict(zoom=want['zoom'], pan_x=pan, pan_y='0.000', filter=filter_word, collapsed=want['collapsed'])
            log.add('state_set', gen=sequence, detail=dict(kind='view_op', op=kind, rows=int(rows), **facts))
            log.add('view_op_drawn', gen=sequence, detail=dict(rows_drawn_model=int(rows), rows=rows, sequence=str(sequence), **facts), step_ms=8)
    return log


def synthetic_viewops_with_extra():
    """A viewops log with one more zoom call than planned, drawn like the rest."""
    log = synthetic_viewops()
    last = max(e['gen'] for e in log.events if e.get('event') == 'view_op_call')
    zoom = next(e for e in log.events if e.get('event') == 'view_op_drawn' and e['detail'].get('rows') is not None)
    log.add('view_op_call', gen=last + 1, detail=dict(op='zoom'))
    log.add('state_set', gen=last + 1, detail=dict(kind='view_op', op='zoom', rows=int(zoom['detail']['rows']), **{k: zoom['detail'][k] for k in FINGERPRINT}))
    log.add('view_op_drawn', gen=last + 1, detail=dict(zoom['detail'], sequence=str(last + 1)), step_ms=8)
    return log.close()


def synthetic_scroll(windows=2, fps=60, seconds=2.0, extent=3080.0, viewport=616.0, displaced=0.0, drawn=True, axes=('vertical', 'horizontal'), fits=(), fit_extent=104.0):
    """`fits` names the axes whose content fits the viewport: the driver says so and drives nothing there."""
    log = Synthetic()
    log.ticks()
    window_id = 0
    for _ in range(windows):
        for axis in axes:
            window_id += 1
            log.add('scroll_window_start', detail=dict(axis=axis, window_id=window_id, seconds=seconds))
            if axis in fits:
                log.add('scroll_unavailable', gen=f'{window_id}:{axis}:1', detail=dict(axis=axis, window_id=window_id, extent=fit_extent, viewport=700.0))
                log.add('scroll_window_end', detail=dict(axis=axis, window_id=window_id))
                continue
            span = extent - viewport
            for seq in range(1, int((seconds + 0.5) * fps) + 1):
                logical = FRAME_STEP * seq
                expected = wrap_offset(logical, span)
                gen = f'{window_id}:{axis}:{seq}'
                record = dict(axis=axis, window_id=window_id, seq=seq, logical_distance=logical, expected_effective_offset=expected, extent=extent, viewport=viewport)
                log.add('scroll_request', gen=gen, detail=record, step_ms=1000 / fps)
                if drawn:
                    shown = dict(record, geometry_drawn_offset=expected + displaced, offset=f'{expected + displaced:.3f}')
                    log.add('frame' if not displaced else 'frame_mismatch', gen=gen, detail=shown, step_ms=1)
            log.add('scroll_window_end', detail=dict(axis=axis, window_id=window_id))
    return log


def synthetic_bf2(keys, broken=None):
    log = Synthetic()
    log.ticks()
    for index, key in enumerate(keys, 1):
        log.add('select_call', gen=index, detail=dict(subject=key))
        log.add('state_set', gen=index, detail=dict(kind='selection', sections=9))
        if broken != 'undrawn':
            log.add('detail_drawn', gen=index, detail=dict(subject=key, loading='false', sections=9 if broken != 'sections' else 8), step_ms=20)
    return log


def synthetic_bf7(revisions, selected='PERF-1', drawn_selected=None, started=None):
    log = Synthetic()
    log.ticks()
    started = {}
    for revision in revisions:
        started[revision] = log.clock + 1
        log.add('commit_seen', gen=revision, detail=dict(revision=revision, selected=selected), step_ms=5)
        if drawn_selected != 'undrawn':
            log.add('commit_drawn', gen=revision, detail=dict(revision=str(revision), selected=drawn_selected or selected), step_ms=20)
    return log, started


def self_test():
    """Run the analyzer and the evidence capture on frozen logs. Returns the lines it checked, or raises AssertionError."""
    done = []

    def check(condition, what):
        assert condition, f'self-test failed: {what}'
        done.append(what)

    def invalid(analysis, *reasons):
        return all(r in analysis['invalid'] for r in reasons) and analysis['values'] is None

    # BF4: planned operations, content facts, entry timestamp, timeouts, lateness, drops.
    good = synthetic_viewops()
    ok = analyze_bf4(good.close(), RECORD, 'run-1')
    check(not ok['invalid'] and ok['values'] is not None and all(len(ok['values'][k]) > 0 for k in PLANNED_VIEW_OPS), 'a correct frozen BF4 log is valid and yields its latencies')
    per_kind = {}
    for call in (e for e in good.events if e.get('event') == 'view_op_call'):
        per_kind[call['detail']['op']] = per_kind.get(call['detail']['op'], 0) + 1
    check(per_kind == {k: 10 for k in ('collapse_all', 'expand_all', 'zoom', 'pan', 'filter')} and PLANNED_VIEW_OPS == per_kind
          and all(len(ok['values'][k]) == 9 for k in PLANNED_VIEW_OPS) and all(len(ok['values'][k] * 7) == 63 for k in PLANNED_VIEW_OPS),
          'BF4: a viewops log has exactly ten calls per kind, each matched to its state and draw; nine are kept per repetition, 63 over seven')
    after_filter = copy.deepcopy(good.events)
    next(e for e in after_filter if e.get('event') == 'view_op_drawn' and e['gen'] == 6)['detail']['filter'] = 'critical only'
    check(invalid(analyze_bf4(after_filter, RECORD), 'CONTENT_MISMATCH'), 'BF4: a filter still on after it was cleared is CONTENT_MISMATCH with no number')
    check(invalid(analyze_bf4(synthetic_viewops_with_extra(), RECORD), 'UNPLANNED_OPERATION'), 'BF4: a twentieth call of a kind (the old 20 per kind) is UNPLANNED_OPERATION, never relabelled as 63')
    check(invalid(analyze_bf4(mutate(good.events, 'content_mismatch_zoom'), RECORD), 'CONTENT_MISMATCH'), 'BF4: a draw of the right generation and rows with the wrong zoom is CONTENT_MISMATCH with no number')
    pan = copy.deepcopy(good.events)
    next(e for e in pan if e.get('event') == 'view_op_drawn' and e['detail']['pan_x'] == '160.000')['detail']['pan_x'] = '0.000'
    check(invalid(analyze_bf4(pan, RECORD), 'CONTENT_MISMATCH'), 'BF4: a draw with the right rows and the wrong pan is CONTENT_MISMATCH with no number')
    check(invalid(analyze_bf4(mutate(good.events, 'dropped_event'), RECORD), 'EVENTS_DROPPED', 'INCOMPLETE_GENERATION'), 'BF4: a dropped event is INVALID with no number')
    check(invalid(analyze_bf4(mutate(good.events, 'late_generation'), RECORD), 'LATE_GENERATION'), 'BF4: a completed but late generation is INVALID with no number')
    check(invalid(analyze_bf4(mutate(good.events, 'timeout_then_draw'), RECORD), 'GENERATION_TIMEOUT'), 'BF4: a generation_timeout stays INVALID though a draw eventually appeared')
    check(invalid(analyze_bf4(mutate(good.events, 'missing_operation'), RECORD), 'MISSING_REQUIRED_OPERATION'), 'BF4: a missing planned operation is INVALID, not any operation')
    wrong_entry = copy.deepcopy(good.events)
    call = next(e for e in wrong_entry if e.get('event') == 'view_op_call')
    call['t_ns'] += 10_000_000_000
    check(invalid(analyze_bf4(wrong_entry, RECORD), 'ORDER_VIOLATION'), 'BF4: a call stamped after its state and draw (not at handler entry) is INVALID')
    check(invalid(analyze_bf4([e for e in good.events if e.get('event') != 'log_closed'], RECORD), 'LOG_NOT_CLOSED'), 'a log with no closing line is INVALID')
    # BF2
    keys = [f'PERF-{n}' for n in range(1, 11)]
    check(not analyze_bf2(synthetic_bf2(keys).close(), keys)['invalid'], 'a correct frozen BF2 log is valid')
    check(invalid(analyze_bf2(synthetic_bf2(keys, 'undrawn').close(), keys), 'INCOMPLETE_GENERATION'), 'BF2: undrawn selections are INVALID with no number')
    check(invalid(analyze_bf2(synthetic_bf2(keys, 'sections').close(), keys), 'CONTENT_MISMATCH'), 'BF2: a draw with another section count than the model assigned is CONTENT_MISMATCH')
    check(invalid(analyze_bf2(synthetic_bf2(keys[:9]).close(), keys), 'MISSING_REQUIRED_OPERATION'), 'BF2: a missing planned selection is INVALID')
    # BF5: the trajectory, the wrap, identities across windows, the mismatch and the zero range.
    scroll = synthetic_scroll(windows=2, seconds=2.0)
    ok = analyze_bf5(scroll.close(), 2.0)
    check(not ok['invalid'] and ok['values'] is not None and len(ok['windows']) == 4 and len({(w['window_id'], w['axis']) for w in ok['windows']}) == 4 and all(w['drawn'] > 0 and w['frames_per_second'] for w in ok['windows']),
          'a correct frozen BF5 log with two axes in two successive windows is valid, with unique identities and frames in every window')
    check(all(w['wrapped_frames'] > 0 for w in ok['windows']), 'BF5: the frames cross the content extent and the actual, expected and requested offsets agree')
    check(invalid(analyze_bf5(synthetic_scroll(windows=1, displaced=7.0).close(), 2.0), 'CONTENT_MISMATCH') and all(w['frames_per_second'] is None for w in analyze_bf5(synthetic_scroll(windows=1, displaced=7.0).close(), 2.0)['windows']),
          'BF5: a deliberate coordinate mismatch is INVALID/CONTENT_MISMATCH with no frames per second')
    check(invalid(analyze_bf5(mutate(scroll.events, 'content_mismatch_frame'), 2.0), 'CONTENT_MISMATCH'), 'BF5: one frame drawn at another coordinate makes the run INVALID')
    check(invalid(analyze_bf5(synthetic_scroll(windows=1, drawn=False).close(), 2.0), 'NO_FRAMES_DRAWN'), 'BF5: requests with no frame are NO_FRAMES_DRAWN and never a rate')
    flat = analyze_bf5(synthetic_scroll(windows=1, extent=500.0, viewport=700.0, drawn=False).close(), 2.0)
    check(invalid(flat, 'NO_FRAMES_DRAWN'), 'BF5: content that does not scroll (no range) yields no frames and no rate')
    fitting = analyze_bf5(synthetic_scroll(windows=1, fits=('horizontal',)).close(), 2.0)
    horizontal = next(w for w in fitting['windows'] if w['axis'] == 'horizontal')
    check(not fitting['invalid'] and fitting['values'] is not None and not horizontal['applicable'] and horizontal['frames_per_second'] is None and horizontal['status'] == 'NOT_APPLICABLE_NO_SCROLL_RANGE'
          and next(w for w in fitting['windows'] if w['axis'] == 'vertical')['frames_per_second'],
          'BF5: an axis whose content fits its viewport has no scroll FPS (not applicable, no number, kept with its extent and viewport) while the other axis is measured')
    check(invalid(analyze_bf5(synthetic_scroll(windows=1, fits=('vertical', 'horizontal')).close(), 2.0), 'NO_SCROLL_EXTENT'), 'BF5: a run in which nothing could scroll is INVALID, never a rate')
    claimed = synthetic_scroll(windows=1, fits=('horizontal',), fit_extent=5000.0)
    check(invalid(analyze_bf5(claimed.close(), 2.0), 'NO_SCROLL_EXTENT'), 'BF5: an axis declared unscrollable although its content is larger than its viewport is INVALID')
    stuck = analyze_bf5(synthetic_scroll(windows=1, extent=680.0, viewport=640.0).close(), 2.0)
    check(invalid(stuck, 'NO_SCROLL_MOVEMENT'), 'BF5: frames whose offset did not change are never counted as scroll frames')
    reused = copy.deepcopy(scroll.events)
    for event in reused:
        if event.get('event') in ('scroll_request', 'frame') and event['detail']['window_id'] == 3:
            event['detail']['window_id'] = 1
            event['gen'] = f"1:{event['detail']['axis']}:{event['detail']['seq']}"
    check(invalid(analyze_bf5(reused, 2.0), 'DUPLICATE_GENERATION'), 'BF5: identities reused across windows are INVALID')
    delayed = copy.deepcopy(scroll.events)
    late_frame = next(e for e in delayed if e.get('event') == 'frame' and e['detail']['window_id'] == 1)
    other_window = next(e for e in delayed if e.get('event') == 'scroll_window_start' and e['detail']['window_id'] == 3)
    late_frame['t_ns'] = other_window['t_ns'] + 1_000_000
    after = analyze_bf5(delayed, 2.0)
    third = next(w for w in after['windows'] if w['window_id'] == 3)
    base = next(w for w in ok['windows'] if w['window_id'] == 3)
    check(third['drawn'] == base['drawn'], 'BF5: a delayed stamp of an earlier window does not complete a request of a later one')
    # BF7
    log, started = synthetic_bf7([11, 12, 13, 14, 15])
    ok = analyze_bf7(log.close(), [11, 12, 13, 14, 15], 'PERF-1', started)
    check(not ok['invalid'] and ok['values'] is not None, 'a correct frozen BF7 log is valid')
    log, started = synthetic_bf7([11, 12, 13, 14, 15], drawn_selected='PERF-2')
    check(invalid(analyze_bf7(log.close(), [11, 12, 13, 14, 15], 'PERF-1', started), 'CONTENT_MISMATCH'), 'BF7: the expected revision drawn with another selection is CONTENT_MISMATCH')
    log, started = synthetic_bf7([11], drawn_selected='none')
    check(invalid(analyze_bf7(log.close(), [11], 'PERF-1', started), 'CONTENT_MISMATCH'), 'BF7: the expected revision drawn with no selection is CONTENT_MISMATCH')
    log, started = synthetic_bf7([11], drawn_selected='undrawn')
    check(invalid(analyze_bf7(log.close(), [11], 'PERF-1', started), 'INCOMPLETE_GENERATION'), 'BF7: a commit never drawn is INVALID')
    # BF1
    first = Synthetic()
    first.ticks()
    spawn = first.clock - 50_000_000
    first.add('first_draw', gen='run-1', detail=dict(model_rows=110, viewport_rows_drawn=20), step_ms=40)
    check(not analyze_bf1(first.close(), 110, spawn)['invalid'], 'a correct frozen BF1 log is valid')
    check(invalid(analyze_bf1(first.events, 99, spawn), 'CONTENT_MISMATCH'), 'BF1: another row count than the plan has is CONTENT_MISMATCH')
    dropped = Synthetic()
    dropped.ticks()
    check(invalid(analyze_bf1(dropped.close(dropped=3), 110, 0), 'EVENTS_DROPPED', 'FIRST_DRAW_NOT_REACHED'), 'BF1: no first draw and dropped events are INVALID')
    # BF6: resident memory of the live app and its helper.
    def ps(output, code=0):
        return lambda argv, **_: subprocess.CompletedProcess(argv, code, output, '')
    both = sample_rss(100, 200, ps('100 2048\n200 4096\n'))
    check(both['ok'] and both['app'] == 2048 * 1024 and both['helper'] == 4096 * 1024 and both['app'] + both['helper'] == 6144 * 1024, 'BF6: a sample with both exact pids sums the app and the helper')
    for what, sample in (('a nonzero ps exit', sample_rss(100, 200, ps('100 2048\n', 1))), ('a missing helper pid', sample_rss(100, None, ps(''))),
                         ('output missing one pid', sample_rss(100, 200, ps('100 2048\n'))), ('an unknown app pid', sample_rss(None, 200, ps(''))),
                         ('unparseable output', sample_rss(100, 200, ps('garbage here now\n')))):
        check(not sample['ok'] and (sample['app'] is None or sample['helper'] is None or sample['reasons']) and 'ok' in sample, f'BF6: {what} is a failed sample, never a zero')
    check(sample_rss(100, 200, ps('100 2048\n', 1))['argv'] == ['/bin/ps', '-o', 'pid=,rss=', '-p', '100,200'] and sample_rss(100, 200, ps('100 2048\n', 1))['returncode'] == 1, 'BF6: a failed sample keeps the ps argv and return code')
    session = Synthetic()
    session.ticks()
    for kind in ('detail_drawn', 'view_op_drawn', 'frame'):
        session.add(kind, gen=1)
    good_samples = [dict(sample_rss(100, 200, ps('100 2048\n200 4096\n')), t=n) for n in range(12)]
    ok = analyze_bf6(session.close(), good_samples, 60)
    check(not ok['invalid'] and ok['values']['peak_bytes'] == 6144 * 1024, 'BF6: valid samples give a peak and a growth')
    for what, bad in (('a failed sample', good_samples[:6] + [sample_rss(100, 200, ps('100 2048\n'))] + good_samples[7:]),
                      ('a missing helper', good_samples[:6] + [sample_rss(100, None, ps(''))] + good_samples[7:]),
                      ('too few samples', good_samples[:5]),
                      ('a replaced helper', good_samples[:6] + [dict(good_samples[0], helper_pid=201)] + good_samples[7:])):
        analysis = analyze_bf6(session.events, bad, 60)
        check(analysis['invalid'] and analysis['values'] is None, f'BF6: {what} is INVALID with no peak or growth')
    # The gate and the aggregates: an invalid run contributes nothing, a failed control withholds everything.
    valid_run = dict(warm_up=False, repetition=1, invalid=[], values=dict(first_draw_ms_from_spawn=300.0, first_draw_ms_since_process_entry=250.0), raw={})
    bad_run = dict(warm_up=False, repetition=2, invalid=['CONTENT_MISMATCH'], values=None, raw=dict(first_draw_ms_from_spawn=1.0))
    check(aggregate_bf1([valid_run], 1)['qualified'] and not aggregate_bf1([valid_run, bad_run], 2)['qualified'] and aggregate_bf1([valid_run, bad_run], 2)['from_spawn'] is None,
          'an INVALID kept run keeps its raw observations and contributes no number: the aggregate is not qualified')
    measured = dict(BF1=dict(B100=dict(aggregate=aggregate_bf1([valid_run], 1), runs=[valid_run])))
    check(gates(measured, dict(verdict='CONTROL_PASS')) == {'BF1 B100: max first draw <= 1000 ms': 'MEETS_TARGET'}, 'a valid aggregate is judged against its contract target')
    slow = dict(valid_run, values=dict(first_draw_ms_from_spawn=1500.0, first_draw_ms_since_process_entry=1400.0))
    check(list(gates(dict(BF1=dict(B100=dict(aggregate=aggregate_bf1([slow], 1)))), dict(verdict='CONTROL_PASS')).values()) == ['MISSES_TARGET'], 'a threshold miss is a miss, apart from an invalid instrument')
    check(list(gates(dict(BF1=dict(B100=dict(aggregate=aggregate_bf1([bad_run], 1)))), dict(verdict='CONTROL_PASS')).values()) == ['NOT_EVALUATED_INVALID'], 'an invalid instrument is not evaluated against the target, never a miss or a pass')
    withhold(measured, dict(verdict='CONTROL_FAIL'))
    check(measured['BF1']['B100']['aggregate']['qualified'] is False and 'from_spawn' not in measured['BF1']['B100']['aggregate']
          and list(gates(measured, dict(verdict='CONTROL_FAIL')).values()) == ['NOT_EVALUATED_CONTROL_FAIL'], 'a failed control means no qualifying aggregate is published and the gate is not evaluated')
    # The evidence capture: distinct persistent paths, intact bytes, a failing launch keeps its command and output.
    with tempfile.TemporaryDirectory(prefix='dpm-gantt-selftest-') as scratch:
        root = Path(scratch)
        app = root / 'Fake.app/Contents/MacOS'
        app.mkdir(parents=True)
        (app / 'dpm-observer').write_text('#!/bin/sh\nwhile [ $# -gt 0 ]; do case "$1" in --measure-log) log="$2"; shift;; --state-file) st="$2"; shift;; --fake-exit) code="$2"; shift;; esac; shift; done\n'
                                          'printf \'{"event":"log_opened","measure_id":"m","pid":1,"t_ns":1}\\n\' > "$log"\nprintf \'{"connection":"connected"}\' > "$st"\n'
                                          'printf \'{"event":"log_closed","dropped":0,"measure_id":"m","t_ns":2}\\n\' >> "$log"\necho out-text\necho err-text >&2\nexit ${code:-0}\n')
        (app / 'dpm-observer').chmod(0o755)
        saved = CAPTURE.root, list(CAPTURE.launches)
        CAPTURE.root = root / 'report.d'
        CAPTURE.root.mkdir()
        try:
            first_run = Launched(root / 'Fake.app', root, 'bf4-B100', ['--page', 'gantt'])
            check(first_run.wait_event('log_closed', 10) is not None, 'the launcher waits on the log by event, not by sleeping')
            first_run.process.wait(timeout=10)  # the fake app ends by itself; it is not to be interrupted while it writes its output
            first_run.close()
            before = {p.name: p.read_bytes() for p in first_run.dir.iterdir()}
            second_run = Launched(root / 'Fake.app', root, 'bf4-B100', ['--page', 'gantt'])
            second_run.wait_event('log_closed', 10)
            second_run.process.wait(timeout=10)
            second_run.close()
            failing = Launched(root / 'Fake.app', root, 'bf4-B100', ['--page', 'gantt', '--fake-exit', '3'])
            failing.wait_event('log_closed', 10)
            failing.process.wait(timeout=10)
            failing.close()
            failing.finalize(failure='the run did not complete')
            check(first_run.dir != second_run.dir and first_run.dir.exists() and second_run.dir.exists(), 'two launches of the same budget and dataset keep distinct persistent paths')
            check({p.name: p.read_bytes() for p in first_run.dir.iterdir()} == before and (first_run.dir / 'stdout.txt').read_text() == 'out-text\n' and (first_run.dir / 'stderr.txt').read_text() == 'err-text\n',
                  'the first launch keeps its original bytes, stdout and stderr after later launches and after the command exited')
            hashes = json.loads((first_run.dir / 'sha256.json').read_text())
            check(set(hashes) == {'argv.json', 'log.jsonl', 'state.json', 'stdout.txt', 'stderr.txt', 'outcome.json'} and all(sha256(first_run.dir / n) == h for n, h in hashes.items()), 'the sha256 of each kept file is recorded and equals the file')
            outcome = json.loads((failing.dir / 'outcome.json').read_text())
            argv = json.loads((failing.dir / 'argv.json').read_text())
            check(outcome['returncode'] == 3 and outcome['failure'] == 'the run did not complete' and argv['argv'] == failing.command and '--fake-exit' in argv['argv'] and (failing.dir / 'stderr.txt').read_text() == 'err-text\n',
                  'a failing launch keeps its exact command, its raw output and its process outcome')
            # The report embeds each launch's hashes; the final per-launch loop of main() must change no byte after that.
            CAPTURE.launches[:] = [first_run, second_run, failing]
            embedded = [launch.evidence() for launch in CAPTURE.launches]
            for launch in CAPTURE.launches:
                launch.finalize()
            check(all(item['sha256'] == {p.name: sha256(p) for p in sorted(launch.dir.iterdir()) if p.name != 'sha256.json'} for item, launch in zip(embedded, CAPTURE.launches))
                  and failing.ended_at == json.loads((failing.dir / 'outcome.json').read_text())['ended_at'],
                  'finalization is idempotent: every embedded file hash equals the final on-disk bytes, one failing launch included, and the process end is recorded once')
        finally:
            CAPTURE.root, CAPTURE.launches[:] = saved[0], saved[1]
    return done


# --------------------------------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--probe', action='store_true', help='add the 5000-task scale probe (a design proposal, not in the contract)')
    parser.add_argument('--session-seconds', type=float, default=60.0,
                        help='the scripted session of the memory budget; the record says 300 s, and the shorter default is stated as a deviation in the output')
    parser.add_argument('--repetitions', type=int, default=REPETITIONS)
    parser.add_argument('--skip', action='append', default=[], help='skip a budget by name (recorded, never silent)')
    parser.add_argument('--self-test', action='store_true', help='run the analyzer and the evidence capture on frozen logs and exit')
    args = parser.parse_args()
    if args.self_test:
        for line in self_test():
            print(f'PASS: {line}')
        return 0
    if args.out is None:
        parser.error('--out is required')
    if sys.platform != 'darwin':
        print(f'SKIPPED: the observer is a macOS app and this is {sys.platform}; nothing was measured and nothing is claimed.')
        return 0
    import build_native
    source = build_native.target_directory() / 'native/DPMObserver.app'
    if not (source / 'Contents/MacOS/dpm-observer').is_file():
        print(f'FAILED: no packaged observer at {source}; build it first (the harness native-build step)', file=sys.stderr)
        return 1
    # Every launch keeps its raw evidence in a directory named after the report, next to it.
    args.out.parent.mkdir(parents=True, exist_ok=True)
    CAPTURE.root = args.out.parent / args.out.stem
    CAPTURE.root.mkdir(parents=True, exist_ok=True)
    failures, measured, deviations = [], {}, []
    started = datetime.now(timezone.utc).isoformat()
    with tempfile.TemporaryDirectory(prefix='dpm-gantt-') as scratch:
        directory = Path(scratch)
        bundle = directory / 'DPMObserver.app'
        import shutil
        shutil.copytree(source, bundle, symlinks=True)
        executable, helper = bundle / 'Contents/MacOS/dpm-observer', bundle / 'Contents/Helpers/dpm-native'
        datasets = Datasets(directory)
        print('the analyzer on frozen logs before anything is launched', flush=True)
        for line in self_test():
            print(f'  ok: {line}', flush=True)
        print('negative control (NC-0 and NC-1) before any number is kept', flush=True)
        try:
            control, control_failures = controls(bundle, directory, datasets)
        except RunFailed as error:
            # A control that could not be run is a failed control: nothing is qualified, and the reason is kept.
            control_failures = [f'a control could not be run: {error}']
            control = dict(results=[], verdict='CONTROL_FAIL', failures=control_failures)
        failures += [f'control {f}' for f in control_failures]
        reps = args.repetitions
        names = ['B100', 'B1000'] + (['B5000'] if args.probe else [])

        def budget(key, label, work, aggregate, items):
            if key in args.skip:
                measured[key] = dict(status='skipped', reason='--skip')
                return
            measured[key] = {}
            for name in items:
                datasets.need(name)
                print(f'{label} on {name}', flush=True)
                outcomes = repeat(f'{label} {name}', lambda name=name: work(bundle, directory, datasets, name), failures, reps)
                measured[key][name] = dict(aggregate=aggregate(outcomes, reps), runs=outcomes)

        budget('BF1', 'BF1 first draw', bf1, aggregate_bf1, names)
        budget('BF2', 'BF2 selection to Detail', bf2, aggregate_bf2, ['B100', 'B1000'])
        budget('BF4', 'BF4 view operations', bf4, aggregate_bf4, ['B1000', 'D1000'])
        budget('BF5', 'BF5 scroll frames', bf5, aggregate_bf5, ['B100', 'B1000', 'D1000'] + (['B5000'] if args.probe else []))
        budget('BF6', 'BF6 resident memory', lambda b, d, ds, n: bf6(b, d, ds, n, args.session_seconds), aggregate_bf6, ['B100', 'B1000', 'D1000'] + (['B5000'] if args.probe else []))
        budget('BF7', 'BF7 external commit drawn', bf7, aggregate_bf7, ['C1000'])
        if 'BF4' in measured and 'BF5' in measured and 'status' not in measured.get('BF4', {}):
            measured['BF8'] = {name: dict(bf4=aggregate_bf8(measured['BF4'][name]['runs']) if name in measured['BF4'] else None,
                                          bf5=aggregate_bf8(measured['BF5'][name]['runs']) if name in measured['BF5'] else None) for name in ('B1000', 'D1000')}
        rendered = []
        if os.environ.get('DPM_RENDER_OUT'):
            try:
                rendered = render_dense(bundle, directory, datasets, Path(os.environ['DPM_RENDER_OUT']))
            except RunFailed as error:
                failures.append(str(error))
        if args.session_seconds != 300:
            deviations.append(f'the memory session was {args.session_seconds:g} s per run; the record defines 300 s (run with --session-seconds 300 for the record\'s definition)')
        if control['verdict'] != 'CONTROL_PASS':
            deviations.append('the negative control did not pass, so no BF number is qualified (record J5.5): every aggregate is withheld')
        withhold(measured, control)
        verdicts = gates(measured, control)
        misses = [name for name, verdict in verdicts.items() if verdict == 'MISSES_TARGET']
        document = dict(
            schema='dpm.gantt-measurements.v2', started_at=started, finished_at=datetime.now(timezone.utc).isoformat(), command=sys.argv,
            method='accepted design record FEAT-05 correction 2, section c.5 (sha256 116386ab1425bc5bb554c63d3afbf3e05605efb537bdeecfa6749262f43c7e7e), with the FEAT-20 scroll method clarification below',
            scroll_method=SCROLL_METHOD,
            hardware=hardware(),
            binaries=dict(observer_executable_sha256=sha256(executable), helper_sha256=sha256(helper), bundle_copied_from=str(source)),
            configuration=dict(repetitions_kept=reps, warm_ups_discarded=1, session_seconds=args.session_seconds, probe=args.probe, clock='CLOCK_UPTIME_RAW in the app; the harness reads the same clock for BF1 and BF7',
                               frame_window_seconds=5.0, first_draw_timeout_s=FIRST_DRAW_TIMEOUT, generation_timeout_s=GENERATION_TIMEOUT, frame_step_points=FRAME_STEP,
                               planned_view_operations=PLANNED_VIEW_OPS, scroll_trajectory='wrap over the scroll range (extent less viewport); expected effective offset worked out by the driver, drawn offset read from the geometry',
                               surface='SwiftUI Canvas of the Gantt timeline (S4 draw pass); the Detail stamp is a sibling Canvas in the Detail pane'),
            datasets=datasets.records, stores_created=datasets.stores,
            commands=dict(app_launch_template='dpm-observer --database STORE --page PAGE [--select-key K] --state-file F --measure-log L --measure-id UUID [--measure-script S --measure-keys KEYS --measure-seconds N --measure-delay N] [--measure-freeze SURFACES] [--measure-displace POINTS]',
                          generator='scripts/smoke_performance.py generated(count, shape) with the overlays of scripts/measure_gantt.py (DS-H, DS-D)',
                          exact_argv_per_launch='in each launch directory under the evidence directory (argv.json)'),
            evidence=dict(directory=str(CAPTURE.root), layout='one subdirectory per launch: <name>-<measure id>/{argv.json,log.jsonl,state.json,stdout.txt,stderr.txt,outcome.json,sha256.json}', launches=len(CAPTURE.launches)),
            controls=control, targets=TARGETS, measured=measured, gates=verdicts, threshold_misses=misses, deviations=deviations, failures=failures, rendered_png=rendered,
            not_measured=dict(BF3='operation outcome: belongs to the operations panel (FEAT-40)', BF9='readability is a reviewer check', ui_observation='NOT ASSESSED by this script'),
        )
    for launch in CAPTURE.launches:
        launch.finalize()
    args.out.write_text(json.dumps(document, indent=2) + '\n')
    print(f'wrote {args.out} (raw evidence of {len(CAPTURE.launches)} launches in {CAPTURE.root})')
    if misses:
        print('TARGET MISSES (reported apart from the validity of the instrument): ' + '; '.join(misses))
    if failures:
        print('FAILED: an instrument is missing, a run was INVALID or a run did not complete:', file=sys.stderr)
        for failure in failures:
            print(f'  - {failure}', file=sys.stderr)
        return 1
    print('complete: every run was valid; the reviewer judges the numbers against the targets')
    return 0


if __name__ == '__main__':
    sys.exit(main())
