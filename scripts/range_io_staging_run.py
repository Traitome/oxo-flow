#!/usr/bin/env python3
"""Measurement driver for oxo-flow issue #1250 — range I/O and object-storage
staging (ROADMAP 12.1 row "Range I/O and object-storage staging").

Runs the REAL oxo-flow binary against a live S3-compatible server (RustFS;
MinIO's community server is no longer distributed — see
crates/oxo-flow-cli/tests/remote_staging.rs) and emits RAW numbers for four
measurements:

  M1 streaming end-to-end   throughput (wall s, MiB/s) and peak RSS for the
                            `eager` and `stream` staging modes over the same
                            object.
  M2 interrupt / resume     kill the engine mid-`eager` download and mid-
                            `stream` read; record the leftover artifact and
                            the failure mode, then re-run and assert the
                            resumed output is byte-correct.
  M3 backpressure           slow consumer; record whether producer memory /
                            scratch stays bounded (report a defect if not).
  M4 scratch footprint      peak bytes under .oxo-flow/staged for eager vs
                            stream over one object.

Non-vacuity (FAIL-FIRST): `--self-test` re-runs the same workflow with
`staging_mode` set to `eager` and compares that control's peak scratch against
the previously captured `stream` arm's number, asserting the scratch-footprint
discriminator separates the two staging paths (stream ~0 scratch, eager control
~object size). A harness that always reported ~0 scratch would fail this. It
does not mutate the stream arm; it adds an eager control run.

This file is invoked by scripts/range-io-staging.sh; it does not bring up
the server itself. Exit code is 0 unless --self-test assertions fail.
"""

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
import time

SLOW_SHA = r'''
import sys, time, hashlib
src, dst, rate_mib = sys.argv[1], sys.argv[2], float(sys.argv[3])
h = hashlib.sha256(); total = 0; t0 = time.monotonic()
bs = 1 << 20
with open(src, "rb") as f:
    while True:
        b = f.read(bs)
        if not b:
            break
        h.update(b); total += len(b)
        el = time.monotonic() - t0
        want = total / (rate_mib * 1024 * 1024)
        if want > el:
            time.sleep(want - el)
with open(dst, "w") as g:
    g.write(h.hexdigest() + "\n")
'''


def log(msg):
    sys.stderr.write(msg + "\n")
    sys.stderr.flush()


def vmhwm_kb(pid):
    """Peak (high-water) resident set of `pid` in kB, 0 if gone."""
    try:
        with open("/proc/%d/status" % pid) as f:
            for line in f:
                if line.startswith("VmHWM:"):
                    return int(line.split()[1])
    except (FileNotFoundError, ProcessLookupError):
        return 0
    return 0


def staged_bytes(workdir):
    """Apparent bytes under .oxo-flow/staged (FIFO st_size == 0)."""
    root = os.path.join(workdir, ".oxo-flow", "staged")
    total = 0
    for dp, _dn, fn in os.walk(root):
        for n in fn:
            try:
                total += os.lstat(os.path.join(dp, n)).st_size
            except OSError:
                pass
    return total


def find_part(workdir, min_bytes=0):
    root = os.path.join(workdir, ".oxo-flow", "staged")
    for dp, _dn, fn in os.walk(root):
        for n in fn:
            if n.endswith(".part"):
                try:
                    if os.stat(os.path.join(dp, n)).st_size > min_bytes:
                        return os.path.join(dp, n)
                except OSError:
                    pass
    return None


def find_fifo(workdir):
    root = os.path.join(workdir, ".oxo-flow", "staged")
    for dp, _dn, fn in os.walk(root):
        for n in fn:
            p = os.path.join(dp, n)
            try:
                if stat.S_ISFIFO(os.lstat(p).st_mode):
                    return p
            except OSError:
                pass
    return None


def find_staged_in(workdir):
    """The staged (or FIFO) input under .oxo-flow/staged/in, if any."""
    root = os.path.join(workdir, ".oxo-flow", "staged", "in")
    for dp, _dn, fn in os.walk(root):
        for n in fn:
            if not n.endswith(".meta.json") and not n.endswith(".part"):
                return os.path.join(dp, n)
    return None


def workflow_text(bucket, key, mode, shell):
    return (
        '[workflow]\n'
        'name = "range-io-staging"\n\n'
        '[[rules]]\n'
        'name = "consume"\n'
        'input = ["s3://%s/%s"]\n'
        'output = ["out.txt"]\n'
        'staging_mode = "%s"\n'
        'shell = "%s"\n' % (bucket, key, mode, shell)
    )


def run_once(binp, workdir, env, kill_after=None):
    """Run `oxo-flow run wf.oxoflow`; sample peak RSS + scratch.

    kill_after: optional (predicate, extra_delay_s) — when the predicate
    first becomes true, wait extra_delay_s then SIGKILL the engine.
    """
    t0 = time.monotonic()
    p = subprocess.Popen(
        [binp, "run", "wf.oxoflow"], cwd=workdir, env=env,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    peak_rss = 0
    peak_scr = 0
    killed = False
    kill_reason = ""
    while p.poll() is None:
        peak_rss = max(peak_rss, vmhwm_kb(p.pid))
        peak_scr = max(peak_scr, staged_bytes(workdir))
        if kill_after and not killed:
            pred, delay = kill_after
            hit = pred()
            if hit:
                time.sleep(delay)
                peak_rss = max(peak_rss, vmhwm_kb(p.pid))
                peak_scr = max(peak_scr, staged_bytes(workdir))
                p.kill()
                killed = True
                kill_reason = hit if isinstance(hit, str) else "predicate"
        time.sleep(0.03)
    out, err = p.communicate()
    return {
        "rc": p.returncode,
        "wall_s": round(time.monotonic() - t0, 4),
        "peak_rss_kb": peak_rss,
        "peak_scratch_bytes": peak_scr,
        "killed": killed,
        "kill_reason": kill_reason,
        "stdout": out[-4000:],
        "stderr": err[-4000:],
    }


def read_out_sha(workdir):
    p = os.path.join(workdir, "out.txt")
    if not os.path.exists(p):
        return None
    with open(p) as f:
        data = f.read().strip()
    return data.split()[0] if data else None


def seed_object(s3, bucket, key, path, mib):
    if not os.path.exists(path):
        log("seeding %d MiB object -> %s" % (mib, path))
        with open(path, "wb") as f:
            block = os.urandom(1 << 20)
            for _ in range(mib):
                f.write(block)
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    s3.upload_file(path, bucket, key)
    return h.hexdigest()


def mibps(obj_bytes, wall_s):
    return round(obj_bytes / (1024 * 1024) / wall_s, 2) if wall_s > 0 else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bin")
    ap.add_argument("root", help="scratch root (outside the repo)")
    ap.add_argument("--bucket", default="oxo-1250")
    ap.add_argument("--obj-mib", type=int, default=256)
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()

    try:
        import boto3
        from botocore.config import Config
    except ImportError:
        log("boto3 is required (pip install boto3)")
        return 2

    endpoint = os.environ.get("AWS_ENDPOINT_URL", "http://127.0.0.1:9000")
    s3 = boto3.client(
        "s3", endpoint_url=endpoint,
        aws_access_key_id=os.environ.get("AWS_ACCESS_KEY_ID", "minioadmin"),
        aws_secret_access_key=os.environ.get("AWS_SECRET_ACCESS_KEY", "minioadmin"),
        region_name=os.environ.get("AWS_REGION", "us-east-1"),
        config=Config(s3={"addressing_style": "path"}),
    )
    try:
        s3.create_bucket(Bucket=args.bucket)
    except Exception as e:  # bucket may already exist
        log("create_bucket: %s" % e)

    env = dict(os.environ)
    env.update({
        "AWS_ENDPOINT_URL": endpoint,
        "AWS_ACCESS_KEY_ID": os.environ.get("AWS_ACCESS_KEY_ID", "minioadmin"),
        "AWS_SECRET_ACCESS_KEY": os.environ.get("AWS_SECRET_ACCESS_KEY", "minioadmin"),
        "AWS_REGION": os.environ.get("AWS_REGION", "us-east-1"),
        "OXO_S3_FORCE_PATH_STYLE": "1",
    })

    os.makedirs(args.root, exist_ok=True)
    seeddir = os.path.join(args.root, "seed")
    os.makedirs(seeddir, exist_ok=True)
    obj_bytes = args.obj_mib * 1024 * 1024
    key = "obj-%dMiB.bin" % args.obj_mib
    seed = os.path.join(seeddir, key)
    want_sha = seed_object(s3, args.bucket, key, seed, args.obj_mib)
    log("object sha256=%s (%d bytes)" % (want_sha, obj_bytes))

    # Slow-consumer rate (MiB/s) scaled so a run lasts ~16 s: exercises app
    # backpressure without an unbounded wait. Floor of 4 MiB/s.
    slow_rate = max(4, round(args.obj_mib / 16))

    def make_workdir(name):
        d = os.path.join(args.root, name)
        if os.path.exists(d):
            subprocess.run(["rm", "-rf", d], check=False)
        os.makedirs(d)
        with open(os.path.join(d, "slow_sha.py"), "w") as f:
            f.write(SLOW_SHA)
        return d

    FAST = "sha256sum {input[0]} > {output[0]}"

    def slow(rate):
        return "python3 slow_sha.py {input[0]} {output[0]} %d" % rate

    results = {"endpoint": endpoint, "bin": args.bin,
               "object_bytes": obj_bytes, "object_sha256": want_sha,
               "slow_consumer_mibps": slow_rate, "measurements": {}}

    # ---- M1 + M4: eager vs stream, fast consumer --------------------------
    log("M1/M4: eager vs stream (fast consumer)")
    for mode in ("eager", "stream"):
        wd = make_workdir("m14-%s" % mode)
        with open(os.path.join(wd, "wf.oxoflow"), "w") as f:
            f.write(workflow_text(args.bucket, key, mode, FAST))
        r = run_once(args.bin, wd, env)
        r["ok"] = (r["rc"] == 0 and read_out_sha(wd) == want_sha)
        r["got_sha"] = read_out_sha(wd)
        r["mibps"] = mibps(obj_bytes, r["wall_s"])
        results["measurements"]["m14_%s" % mode] = r
        log("  %-7s rc=%s wall=%ss %.1f MiB/s peak_rss=%d kB peak_scratch=%d B ok=%s"
            % (mode, r["rc"], r["wall_s"], r["mibps"] or -1,
               r["peak_rss_kb"], r["peak_scratch_bytes"], r["ok"]))

    # ---- M2: interrupt / resume -------------------------------------------
    log("M2: interrupt + resume")
    # eager: kill once the .part file passes 4 MiB
    wd = make_workdir("m2-eager")
    with open(os.path.join(wd, "wf.oxoflow"), "w") as f:
        f.write(workflow_text(args.bucket, key, "eager", FAST))
    r_kill = run_once(args.bin, wd, env,
                      kill_after=(lambda: (find_part(wd, 4 << 20) and "part>4MiB"), 0.05))
    part_left = find_part(wd)
    r_kill["leftover_part"] = part_left
    r_kill["interrupted_mid_download"] = part_left is not None
    r_res = run_once(args.bin, wd, env)
    r_kill["resume_rc"] = r_res["rc"]
    r_kill["resume_wall_s"] = r_res["wall_s"]
    r_kill["resume_peak_rss_kb"] = r_res["peak_rss_kb"]
    r_kill["resume_peak_scratch_bytes"] = r_res["peak_scratch_bytes"]
    r_kill["resume_ok"] = (r_res["rc"] == 0 and read_out_sha(wd) == want_sha)
    r_kill["resume_sha"] = read_out_sha(wd)
    si = find_staged_in(wd)
    r_kill["resume_leftover_part"] = find_part(wd)
    r_kill["resume_staged_size"] = os.path.getsize(si) if si else None
    results["measurements"]["m2_eager"] = r_kill
    log("  eager kill rc=%s mid_download=%s resume_ok=%s"
        % (r_kill["rc"], r_kill["interrupted_mid_download"], r_kill["resume_ok"]))
    # stream: slow consumer keeps it reading; kill ~1.2 s after the FIFO shows
    wd = make_workdir("m2-stream")
    with open(os.path.join(wd, "wf.oxoflow"), "w") as f:
        f.write(workflow_text(args.bucket, key, "stream", slow(slow_rate)))
    t_start = time.monotonic()

    def fifo_then_1s():
        if find_fifo(wd) and (time.monotonic() - t_start) > 1.2:
            return "fifo+1s"
        return None

    r_kill = run_once(args.bin, wd, env, kill_after=(fifo_then_1s, 0.05))
    fifo_left = find_fifo(wd)
    r_kill["leftover_fifo"] = fifo_left
    r_kill["interrupted_mid_stream"] = fifo_left is not None
    r_res = run_once(args.bin, wd, env)
    r_kill["resume_rc"] = r_res["rc"]
    r_kill["resume_wall_s"] = r_res["wall_s"]
    r_kill["resume_peak_rss_kb"] = r_res["peak_rss_kb"]
    r_kill["resume_peak_scratch_bytes"] = r_res["peak_scratch_bytes"]
    r_kill["resume_ok"] = (r_res["rc"] == 0 and read_out_sha(wd) == want_sha)
    r_kill["resume_sha"] = read_out_sha(wd)
    si = find_staged_in(wd)
    r_kill["resume_leftover_fifo"] = find_fifo(wd)
    r_kill["resume_staged_is_fifo"] = bool(
        si and stat.S_ISFIFO(os.lstat(si).st_mode))
    results["measurements"]["m2_stream"] = r_kill
    log("  stream kill rc=%s fifo_left=%s resume_ok=%s"
        % (r_kill["rc"], fifo_left is not None, r_kill["resume_ok"]))

    # ---- M3: backpressure (slow consumer) ---------------------------------
    log("M3: backpressure (slow consumer, %d MiB/s)" % slow_rate)
    for mode in ("stream", "eager"):
        wd = make_workdir("m3-%s" % mode)
        with open(os.path.join(wd, "wf.oxoflow"), "w") as f:
            f.write(workflow_text(args.bucket, key, mode, slow(slow_rate)))
        r = run_once(args.bin, wd, env)
        r["ok"] = (r["rc"] == 0 and read_out_sha(wd) == want_sha)
        results["measurements"]["m3_%s" % mode] = r
        log("  %-7s wall=%ss peak_rss=%d kB peak_scratch=%d B ok=%s"
            % (mode, r["wall_s"], r["peak_rss_kb"], r["peak_scratch_bytes"], r["ok"]))

    # ---- FAIL-FIRST self-test ---------------------------------------------
    if args.self_test:
        log("SELF-TEST: eager control run vs captured stream scratch must differ")
        obj = obj_bytes
        m_stream = results["measurements"]["m14_stream"]["peak_scratch_bytes"]
        wd = make_workdir("selftest-eager-control")
        # Eager control run (the same object, staging_mode = eager): the
        # captured stream arm reported ~0 scratch, so a full-object control
        # proves the measurement sees the staging path (a harness blind to it
        # would report ~0 for both). It does not mutate the stream arm.
        with open(os.path.join(wd, "wf.oxoflow"), "w") as f:
            f.write(workflow_text(args.bucket, key, "eager", FAST))
        r = run_once(args.bin, wd, env)
        r["ok"] = (r["rc"] == 0 and read_out_sha(wd) == want_sha)
        results["measurements"]["selftest_eager_control"] = r
        pe = r["peak_scratch_bytes"]
        a = m_stream < 0.02 * obj
        b = pe > 0.9 * obj
        results["self_test"] = {"stream_scratch": m_stream,
                                "eager_control_scratch": pe,
                                "stream_bounded": a, "eager_control_full": b,
                                "pass": bool(a and b)}
        log("  stream scratch=%d B (<2%% of obj: %s) eager-control scratch=%d B (>90%%: %s) => %s"
            % (m_stream, a, pe, b, "PASS" if (a and b) else "FAIL"))

    rp = os.path.join(args.root, "results.json")
    with open(rp, "w") as f:
        json.dump(results, f, indent=2)
    log("raw results -> %s" % rp)

    print("=== range-io-staging raw summary ===")
    for k, v in sorted(results["measurements"].items()):
        print("%-22s rc=%s wall=%ss mibps=%s rss_kb=%s scratch_B=%s ok=%s"
              % (k, v["rc"], v["wall_s"], v.get("mibps"), v["peak_rss_kb"],
                 v["peak_scratch_bytes"], v.get("ok")))
    if args.self_test:
        st = results["self_test"]
        print("self_test pass=%s stream_scratch=%s eager_control_scratch=%s"
              % (st["pass"], st["stream_scratch"], st["eager_control_scratch"]))
        return 0 if st["pass"] else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
