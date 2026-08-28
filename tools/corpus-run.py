import json, os, subprocess, sys, tempfile

MODES = ["fast", "quality", "strip"]

# macOS has no timeout(1). A run that hangs must still be scored, and a whole
# corpus run once reported 1,194 "command not found" errors because the
# watchdog was assumed rather than checked.
WATCHDOG = ["perl", "-e", "alarm shift; exec @ARGV"]
LIMIT = "20"


def decodes(path, scratch):
    # Ask the system decoder, not a Python imaging library. The defect class
    # this looks for is a file macOS could open becoming one it cannot, so a
    # more permissive reader would answer the wrong question.
    #
    # A full format conversion, not `-g pixelWidth`: reading a property parses
    # the header only, and answers True for a JPEG truncated to a tenth of its
    # length. Apple's decoder is tolerant and resyncs where ours cannot, so a
    # file that fails this is severely broken rather than merely damaged.
    out = os.path.join(scratch, "decode-probe.png")
    r = subprocess.run(["sips", "-s", "format", "png", "--out", out, path],
                       capture_output=True, text=True)
    ok = r.returncode == 0 and os.path.exists(out) and os.path.getsize(out) > 0
    if os.path.exists(out):
        os.remove(out)
    return ok


def run_one(binary, src, mode, workdir):
    out = os.path.join(workdir, "out-%s%s" % (mode, os.path.splitext(src)[1]))
    cmd = WATCHDOG + [LIMIT, binary, src, "--mode", mode, "--out", out]
    try:
        r = subprocess.run(cmd, capture_output=True, text=True)
        code, err = r.returncode, r.stderr.strip()[:200]
    except OSError as e:
        return {"mode": mode, "error": str(e)}

    wrote = os.path.exists(out) and os.path.getsize(out) > 0
    rec = {
        "mode": mode,
        "exit": code,
        # Python reports a signalled child as the negative signal number, so
        # SIGALRM arrives as -14. The shell's 142 is only what a shell would
        # have printed, and scoring against it marks every hang as a clean run.
        "timed_out": code in (-14, 142),
        "wrote_output": wrote,
        "output_bytes": os.path.getsize(out) if wrote else 0,
        "output_decodes": decodes(out, workdir) if wrote else False,
        "stderr": err,
    }
    if wrote:
        os.remove(out)
    return rec


def main(argv):
    if len(argv) < 3:
        print("usage: python3 tools/corpus-run.py SQUINT_BINARY CORPUS_DIR [-o REPORT]")
        print("")
        print("Runs every corpus file through every mode and reports the files")
        print("that the system decoder could open before and cannot open after.")
        return 2

    binary, corpus = argv[1], argv[2]
    report = argv[4] if len(argv) > 4 and argv[3] == "-o" else "corpus-report.json"

    manifest = json.load(open(os.path.join(corpus, "manifest.json")))
    results, dangerous = [], []

    with tempfile.TemporaryDirectory() as workdir:
        for n, entry in enumerate(manifest, 1):
            src = os.path.join(corpus, entry["file"])
            before = decodes(src, workdir)
            runs = [run_one(binary, src, m, workdir) for m in MODES]
            rec = {"file": entry["file"], "family": entry["family"],
                   "input_decodes": before, "runs": runs}
            results.append(rec)

            for r in runs:
                # The defect the 2026-08-19 corpus caught: a desynced walk
                # yields a partial file, the never-grow check passes because it
                # is smaller, and it is reported as success.
                if before and r.get("exit") == 0 and r.get("wrote_output") \
                        and not r.get("output_decodes"):
                    dangerous.append((entry["file"], r["mode"]))
            if n % 25 == 0:
                print("  %d/%d" % (n, len(manifest)))

    open(report, "w").write(json.dumps(
        {"binary": binary, "corpus": corpus, "results": results,
         "dangerous": [{"file": f, "mode": m} for f, m in dangerous]},
        indent=2, sort_keys=True) + "\n")

    total = len(manifest) * len(MODES)
    timeouts = sum(1 for r in results for x in r["runs"] if x.get("timed_out"))
    print("")
    print("%d files x %d modes = %d invocations" % (len(manifest), len(MODES), total))
    print("timed out: %d" % timeouts)
    print("PARTIAL OUTPUT REPORTED AS SUCCESS: %d" % len(dangerous))
    for f, m in dangerous:
        print("  %s (%s)" % (f, m))
    print("report written to %s" % report)
    return 1 if dangerous else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
