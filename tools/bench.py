#!/usr/bin/env python3
"""Benchmark Grammaria against LanguageTool on the same French text.

Measures wall-clock time for a complete CLI invocation (process start, model
loading, checking and output) -- the way users experience it -- over several
repetitions, and reports median/min/max.
"""
import argparse, os, statistics, subprocess, sys, time, tempfile


def run_once(cmd, stdin_data=None):
    t = time.perf_counter()
    p = subprocess.run(cmd, input=stdin_data, capture_output=True, text=True)
    return time.perf_counter() - t, p


def bench(name, cmd, reps, stdin_data=None):
    times = []
    for _ in range(reps):
        dt, p = run_once(cmd, stdin_data)
        times.append(dt)
    return times


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--text', required=True)
    ap.add_argument('--grammaria', default='./target/release/grammaria')
    ap.add_argument('--lt-jar', default='/tmp/lt/LanguageTool-6.6/languagetool-commandline.jar')
    ap.add_argument('--java', default='/tmp/jre/jdk-21.0.13.1+2-jre/bin/java')
    ap.add_argument('--reps', type=int, default=5)
    a = ap.parse_args()
    text = open(a.text, encoding='utf-8').read()

    ours = bench('grammaria', [a.grammaria, 'check', '--json', a.text], a.reps)
    lt = bench('lt', [a.java, '-jar', a.lt_jar, '-l', 'fr', '--json', a.text], a.reps)

    def stats(ts):
        return f'median {statistics.median(ts)*1000:7.0f} ms  min {min(ts)*1000:7.0f} ms  max {max(ts)*1000:7.0f} ms'

    print(f'Grammaria   : {stats(ours)}')
    print(f'LanguageTool: {stats(lt)}')
    print(f'speed-up (median): {statistics.median(lt)/statistics.median(ours):.2f}x')


if __name__ == '__main__':
    main()
