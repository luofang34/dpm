#!/usr/bin/env python3
"""A fault-injecting relay between a client and the real helper, for qualifying the bridge.

It starts the real helper, relays one request line to it and its one answer line back, and after
`--after N` answered requests injects one fault on the next request, so hello, attach and the first
queries are genuine and the fault arrives mid-session.

Faults (`--mode`):
  swallow     the request IS forwarded and processed, but its answer is never relayed and the proxy
              then waits: a command may have committed and the client hears nothing
  garbage     answer with bytes that are not UTF-8
  truncate    answer with half a line, then exit
  mismatch    answer with a different correlation identifier
  exit        forward the request, write `boom` to standard error, exit with status 7
  oversize    answer with a line far longer than any client bound
  flood       answer with 128 MiB of bytes that never contain a newline, as fast as they can be written,
              then go quiet: the client's deadline must cut it off
  ok-false    answer with ok=false but a result and no error
  ok-true-error  answer with ok=true but an error and no result
  error-api   answer with a correlated refusal made under api version 999
  wrong-kind  answer with a result of another kind than the call asks for (the attach result)
  wrong-api   answer with an envelope of another api version than the one announced
  wrong-protocol  answer in another protocol version than the one negotiated
  ignore-term ignore SIGTERM and the end of input forever, so only SIGKILL stops it
  pass        no fault
This is a test fixture, never shipped.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--mode', required=True)
    parser.add_argument('--after', type=int, default=2)
    parser.add_argument('--real', required=True)
    options, helper_arguments = parser.parse_known_args()
    if options.mode == 'ignore-term':
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    child = subprocess.Popen([options.real, *helper_arguments], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    answered = 0
    attached = None
    out = sys.stdout.buffer
    for request in sys.stdin.buffer:
        child.stdin.write(request)
        child.stdin.flush()
        answer = child.stdout.readline()
        faulty = options.mode != 'pass' and answered >= options.after
        if answered == 1:
            attached = json.loads(answer).get('result')
        if not faulty or options.mode == 'ignore-term':
            out.write(answer)
            out.flush()
            answered += 1
            continue
        if options.mode == 'swallow':
            while True:
                time.sleep(3600)
        if options.mode == 'garbage':
            out.write(b'\xff\xfe not utf-8 at all\n')
        elif options.mode == 'truncate':
            out.write(answer[: max(1, len(answer) // 2)])
            out.flush()
            os._exit(0)
        elif options.mode == 'mismatch':
            frame = json.loads(answer)
            frame['id'] = 'some-other-request'
            out.write(json.dumps(frame).encode() + b'\n')
        elif options.mode in ('ok-false', 'ok-true-error', 'error-api', 'wrong-kind'):
            frame = json.loads(answer)
            refusal = {'api_version': 12, 'code': 'read_only_project', 'message': 'forged'}
            if options.mode == 'ok-false':
                frame['ok'] = False
            elif options.mode == 'ok-true-error':
                frame.pop('result', None)
                frame['error'] = refusal
            elif options.mode == 'error-api':
                frame.pop('result', None)
                frame['ok'] = False
                frame['error'] = {**refusal, 'api_version': 999}
            else:
                frame['result'] = attached
            out.write(json.dumps(frame).encode() + b'\n')
        elif options.mode == 'flood':
            chunk = b'x' * 65536
            for _ in range(2048):
                out.write(chunk)
                out.flush()
            while True:
                time.sleep(3600)
        elif options.mode == 'wrong-api':
            frame = json.loads(answer)
            frame['result']['envelope']['api_version'] = 99
            out.write(json.dumps(frame).encode() + b'\n')
        elif options.mode == 'wrong-protocol':
            frame = json.loads(answer)
            frame['protocol'] = 9
            out.write(json.dumps(frame).encode() + b'\n')
        elif options.mode == 'exit':
            sys.stderr.write('boom\n')
            sys.stderr.flush()
            os._exit(7)
        elif options.mode == 'oversize':
            out.write(b'x' * (5 * 1024 * 1024) + b'\n')
        out.flush()
        answered += 1
    if options.mode == 'ignore-term':
        # The client has closed its end. A well-behaved helper would exit now; this one never does.
        while True:
            time.sleep(3600)
    child.stdin.close()
    child.wait()


if __name__ == '__main__':
    main()
