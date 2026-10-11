"""The workload of the native dependency network: proposed TARGETS and the exact method that measures them.

Budgets (proposed targets, never measurements):
    BN1  first draw of the network page, from the app's spawn to its first draw of the complete graph (every node of the
         plan laid out; the canvas draws those in view): at most 1500 ms
    BN2  a selection requested on the network page to the first draw showing that node focused: at most 250 ms
    BN5  panning the network one step per display frame: at least 30 drawn frames per second over each 5 s window
    BN6  peak resident memory of the app and its helper over the pan session: below 300 MB

Datasets are measure_gantt's generated plans, each a hierarchy of work packages:
    B100   100 tasks, 10 packages (110 nodes), 99 finish-to-start edges (a branch)
    B1000  1000 tasks, 100 packages (1100 nodes), 999 edges (a branch)
    D1000  1000 tasks, 100 packages (1100 nodes), 1994 edges, 249 of them Soft (dense)
Reference hardware: MacBook Pro Mac15,9 (Apple M3 Max, 48 GB) on macOS 27.0; every result records the machine it ran on,
and a run on other hardware is reported as such, never compared as the same. Five repetitions per budget and dataset.

Every run launches the packaged
DPMObserver.app with its measurement log; the app's network canvas writes `network_draw` for every draw pass and the
measurement driver writes each request (`net_select_request`, `net_pan_request`) before it is made, all stamped with
CLOCK_UPTIME_RAW. The analyzer pairs requests with draws from the log alone; a run whose log was not closed cleanly,
dropped an event or never drew is INVALID and gives no number. Memory is sampled with `ps` by process identity.

Usage: python3 scripts/measure_network.py --out DIR [--repetitions N] [--datasets B100,B1000,D1000]
       python3 scripts/measure_network.py --self-test
"""

import argparse
import json
import sys
from pathlib import Path

import build_native
import measure_gantt as mg

TARGETS = {
    'BN1_first_draw_ms': dict(max=1500),
    'BN2_selection_drawn_ms': dict(max=250),
    'BN5_pan_frames_per_second': dict(min=30),
    'BN6_peak_resident_bytes': dict(max=300_000_000),
}
DATASETS = ('B100', 'B1000', 'D1000')
REPETITIONS = 5
PAN_SECONDS = 5.0


def draws(events):
    return [e for e in events if e.get('event') == 'network_draw']


def valid(events):
    """Why the log cannot give numbers, or None: it must be closed with nothing dropped, and the network must have drawn."""
    closed = [e for e in events if e.get('event') == 'log_closed']
    if not closed:
        return 'LOG_NOT_CLOSED'
    if closed[-1].get('dropped', 0):
        return 'EVENTS_DROPPED'
    if not draws(events):
        return 'NETWORK_NEVER_DREW'
    return None


def analyze_first_draw(events, spawn_ns, nodes):
    """BN1: from the spawn to the first draw showing every node."""
    reason = valid(events)
    if reason:
        return dict(status='INVALID', reason=reason)
    complete = [e for e in draws(events) if e['detail'].get('nodes') == nodes]
    if not complete:
        return dict(status='INVALID', reason='NO_COMPLETE_DRAW')
    return dict(status='VALID', value=mg.ms(spawn_ns, complete[0]['t_ns']))


def analyze_selections(events):
    """BN2: each requested selection to the first later draw focused on that node."""
    reason = valid(events)
    if reason:
        return dict(status='INVALID', reason=reason)
    drawn = draws(events)
    values, missing = [], []
    for request in (e for e in events if e.get('event') == 'net_select_request'):
        key = request['detail'].get('key')
        shown = next((d for d in drawn if d['t_ns'] > request['t_ns'] and d['detail'].get('focus') == key), None)
        if shown is None:
            missing.append(key)
        else:
            values.append(mg.ms(request['t_ns'], shown['t_ns']))
    if missing or not values:
        return dict(status='INVALID', reason='SELECTION_NOT_DRAWN', missing=missing)
    return dict(status='VALID', values=values, stats=mg.stats(values))


def analyze_pan(events):
    """BN5: draws whose pan moved, per second of the pan window."""
    reason = valid(events)
    if reason:
        return dict(status='INVALID', reason=reason)
    start = next((e for e in events if e.get('event') == 'net_pan_window_start'), None)
    end = next((e for e in events if e.get('event') == 'net_pan_window_end'), None)
    if start is None or end is None or end['t_ns'] <= start['t_ns']:
        return dict(status='INVALID', reason='NO_PAN_WINDOW')
    inside = [d for d in draws(events) if start['t_ns'] <= d['t_ns'] <= end['t_ns']]
    moved, last = 0, None
    for draw in inside:
        position = (draw['detail'].get('pan_x'), draw['detail'].get('pan_y'))
        if position != last:
            moved += 1
        last = position
    seconds = mg.ms(start['t_ns'], end['t_ns']) / 1000
    requests = sum(1 for e in events if e.get('event') == 'net_pan_request' and start['t_ns'] <= e['t_ns'] <= end['t_ns'])
    if requests == 0:
        return dict(status='INVALID', reason='NO_PAN_REQUESTED')
    return dict(status='VALID', value=moved / seconds, frames=moved, requests=requests, seconds=seconds)


def analyze_memory(samples):
    """BN6: the largest sum of the app's and its helper's resident memory among valid samples."""
    good = [s['app'] + s['helper'] for s in samples if s.get('ok')]
    if not good:
        return dict(status='INVALID', reason='NO_VALID_SAMPLE')
    return dict(status='VALID', value=max(good), samples=len(good))


def verdict(name, values):
    target = TARGETS[name]
    if not values:
        return 'NOT_EVALUATED_NO_VALID_RUN'
    worst = max(values) if 'max' in target else min(values)
    ok = worst <= target['max'] if 'max' in target else worst >= target['min']
    return 'MEETS_TARGET' if ok else 'MISSES_TARGET'


def run_one(bundle, directory, datasets, name, script, samples=None):
    database = datasets.store(name)
    arguments = ['--database', str(database), '--page', 'network', '--measure-script', script]
    if script == 'network-selections':
        arguments += ['--measure-keys', ','.join(datasets.keys(name))]
    if script == 'network-pan':
        arguments += ['--measure-seconds', str(PAN_SECONDS)]
    run = mg.Launched(bundle, directory, f'{script}-{name}', arguments)
    try:
        if samples is None:
            run.wait_event('driver_done', 120)
        else:
            # Memory is sampled by process identity while the driver works, each sample made as the log grows.
            helper = None
            deadline = mg.uptime_ns() + 120 * 10**9
            while not run.by_name.get('driver_done'):
                if mg.uptime_ns() > deadline:
                    raise mg.RunFailed('the pan driver did not finish within 120 s')
                run.wait_activity(0.25)
                run.pump()
                helper = helper or (run.state() or {}).get('helper_pid')
                samples.append(mg.sample_rss(run.process.pid, helper))
                if run.process.poll() is not None:
                    break
        run.quit()
        return run.events, run.t0
    finally:
        run.close()


def measure(out, repetitions, names):
    built = build_native.build(rust=False)
    bundle = built['observer']
    out.mkdir(parents=True, exist_ok=True)
    mg.CAPTURE.root = out
    datasets = mg.Datasets(out)
    measured = {}
    for name in names:
        record = datasets.need(name)
        # The network shows every task, milestone and package of the plan, as many nodes as the Gantt has rows.
        nodes = record['rows_expanded']
        first, selections, pans, memory = [], [], [], []
        for _ in range(repetitions):
            events, t0 = run_one(bundle, out, datasets, name, 'network-first-draw')
            first.append(analyze_first_draw(events, t0, nodes))
            events, _ = run_one(bundle, out, datasets, name, 'network-selections')
            selections.append(analyze_selections(events))
            samples = []
            events, _ = run_one(bundle, out, datasets, name, 'network-pan', samples)
            pans.append(analyze_pan(events))
            memory.append(analyze_memory(samples))
        measured[name] = dict(BN1=first, BN2=selections, BN5=pans, BN6=memory)
    verdicts = {}
    for name, runs in measured.items():
        verdicts[f'BN1 {name}'] = verdict('BN1_first_draw_ms', [r['value'] for r in runs['BN1'] if r['status'] == 'VALID'])
        verdicts[f'BN2 {name}'] = verdict('BN2_selection_drawn_ms', [v for r in runs['BN2'] if r['status'] == 'VALID' for v in r['values']])
        verdicts[f'BN5 {name}'] = verdict('BN5_pan_frames_per_second', [r['value'] for r in runs['BN5'] if r['status'] == 'VALID'])
        verdicts[f'BN6 {name}'] = verdict('BN6_peak_resident_bytes', [r['value'] for r in runs['BN6'] if r['status'] == 'VALID'])
    result = dict(targets=TARGETS, method=__doc__, hardware=mg.hardware(), repetitions=repetitions, datasets=[datasets.need(n) for n in names],
                  measured=measured, verdicts=verdicts)
    (out / 'network-results.json').write_text(json.dumps(result, indent=2, default=str) + '\n')
    return result


def self_test():
    """The analyzer on frozen synthetic logs: a valid log gives its numbers, and an invalid one none."""
    def draw(t, nodes=3, focus='', x=0.0):
        return dict(event='network_draw', t_ns=t, detail=dict(nodes=nodes, focus=focus, pan_x=x, pan_y=0.0))
    closed = dict(event='log_closed', dropped=0, t_ns=10**12)
    log = [draw(1_000_000_000, nodes=2), draw(1_200_000_000), dict(event='net_select_request', t_ns=2_000_000_000, detail=dict(key='PERF-2')),
           draw(2_050_000_000, focus='PERF-2'), dict(event='net_pan_window_start', t_ns=3_000_000_000, detail={}),
           *[dict(event='net_pan_request', t_ns=3_000_000_000 + i * 10_000_000, detail={}) for i in range(100)],
           *[draw(3_000_000_000 + i * 20_000_000, x=float(i)) for i in range(50)], dict(event='net_pan_window_end', t_ns=4_000_000_000, detail={}), closed]
    checks = []
    first = analyze_first_draw(log, 900_000_000, 3)
    checks.append(first == dict(status='VALID', value=300.0))
    selection = analyze_selections(log)
    checks.append(selection['status'] == 'VALID' and selection['values'] == [50.0])
    pan = analyze_pan(log)
    checks.append(pan['status'] == 'VALID' and pan['frames'] == 50 and abs(pan['value'] - 50.0) < 1e-9)
    checks.append(analyze_first_draw(log[:-1], 0, 3) == dict(status='INVALID', reason='LOG_NOT_CLOSED'))
    checks.append(analyze_selections([*log[:3], closed])['status'] == 'INVALID')
    checks.append(analyze_memory([dict(ok=True, app=100, helper=20), dict(ok=False)]) == dict(status='VALID', value=120, samples=1))
    checks.append(verdict('BN5_pan_frames_per_second', [29.0, 60.0]) == 'MISSES_TARGET' and verdict('BN1_first_draw_ms', [800.0]) == 'MEETS_TARGET')
    checks.append(verdict('BN2_selection_drawn_ms', []) == 'NOT_EVALUATED_NO_VALID_RUN')
    assert all(checks), checks
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--repetitions', type=int, default=REPETITIONS)
    parser.add_argument('--datasets', default=','.join(DATASETS))
    parser.add_argument('--self-test', action='store_true')
    options = parser.parse_args()
    if options.self_test:
        print(f'PASS: network analyzer on frozen synthetic logs: {len(self_test())} checks')
        return 0
    if options.out is None:
        parser.error('--out is required unless --self-test is given')
    result = measure(options.out, options.repetitions, [n for n in options.datasets.split(',') if n])
    print(json.dumps(result['verdicts'], indent=2))
    return 0


if __name__ == '__main__':
    sys.exit(main())
